//! Riding out a brief restart of the box service behind a gateway.
//!
//! When the box's HTTP server (`box_http_server.py` on `:9000`) restarts —
//! it deliberately exits and is respawned by its supervisor when it detects
//! a wedged USB context — the port is down for a few seconds. A gated box's
//! gateway answers every request in that window with HTTP 502 and
//! `X-Gateway-Error: upstream_unavailable` (gateway-auth contract §7, the
//! same header the Lager CLI's `handle_gateway_upstream_error` keys on).
//!
//! This module holds the transport-agnostic parts of the retry: the policy,
//! the backoff schedule, and the allowlist of requests that are safe to send
//! again. Both clients run the same decisions; only the sleep differs.

use std::time::Duration;

use serde_json::Value;

use crate::error::{Error, Result};
use crate::wire::{HttpRequest, Method};

/// Response header a Lager gateway sets on errors it generates itself.
pub(crate) const GATEWAY_ERROR_HEADER: &str = "x-gateway-error";
/// Response header naming the upstream the gateway could not reach
/// (e.g. `lager:9000`).
pub(crate) const GATEWAY_UPSTREAM_HEADER: &str = "x-gateway-upstream";
/// `X-Gateway-Error` value for "the gateway is up, the box service is not".
pub(crate) const UPSTREAM_UNAVAILABLE: &str = "upstream_unavailable";

/// True for exactly the gateway's "box service is not answering" response:
/// HTTP 502 with `X-Gateway-Error: upstream_unavailable`. A 502 without the
/// header (the box's own service, or a proxy that is not a Lager gateway)
/// and any other gateway error are left alone.
pub(crate) fn is_upstream_unavailable(status: u16, gateway_error: Option<&str>) -> bool {
    status == 502 && gateway_error.map(str::trim) == Some(UPSTREAM_UNAVAILABLE)
}

/// The human-readable message out of a gateway error body: the JSON
/// `error`/`message` field when there is one, the raw text otherwise.
pub(crate) fn gateway_message(body: &str) -> String {
    if let Ok(v) = serde_json::from_str::<Value>(body) {
        if let Some(msg) = v
            .get("error")
            .or_else(|| v.get("message"))
            .and_then(Value::as_str)
        {
            return msg.to_string();
        }
    }
    let body = body.trim();
    if body.is_empty() {
        "HTTP 502 upstream_unavailable (empty body)".to_string()
    } else {
        body.to_string()
    }
}

/// The error for one upstream-unavailable response, before the retry loop
/// fills in the attempt count. The gateway's body becomes the cause.
pub(crate) fn upstream_error(box_host: &str, upstream: Option<String>, body: &str) -> Error {
    Error::BoxServiceUnavailable {
        box_host: box_host.to_string(),
        upstream,
        attempts: 1,
        elapsed: Duration::ZERO,
        retry_safe: false,
        cause: Box::new(Error::Box {
            status: 502,
            message: gateway_message(body),
        }),
    }
}

/// What the retry loop does with one attempt's outcome.
pub(crate) enum Step<T> {
    /// Done: return this to the caller.
    Return(Result<T>),
    /// Sleep this long, then send the request again.
    Retry(Duration),
}

/// Decide what follows attempt number `attempt` (1-based) of `req`.
/// Only [`Error::BoxServiceUnavailable`] is ever retried; when giving up,
/// the error is stamped with the attempts made and the time spent.
pub(crate) fn step<T>(
    policy: &RetryPolicy,
    req: &HttpRequest,
    attempt: u32,
    elapsed: Duration,
    outcome: Result<T>,
) -> Step<T> {
    let Err(Error::BoxServiceUnavailable {
        box_host,
        upstream,
        cause,
        ..
    }) = outcome
    else {
        return Step::Return(outcome);
    };
    let retry_safe = is_retry_safe(req);
    if retry_safe {
        if let Some(delay) = policy.delay_before(attempt, elapsed) {
            log::warn!(
                "lager: {box_host}: box service behind the gateway is not answering \
                 (HTTP 502 upstream_unavailable) on {} {}; retry {attempt} in {:.1}s",
                method_name(req.method),
                req.path,
                delay.as_secs_f64(),
            );
            return Step::Retry(delay);
        }
    }
    Step::Return(Err(Error::BoxServiceUnavailable {
        box_host,
        upstream,
        attempts: attempt,
        elapsed,
        retry_safe,
        cause,
    }))
}

fn method_name(m: Method) -> &'static str {
    match m {
        Method::Get => "GET",
        Method::Post => "POST",
        Method::Put => "PUT",
        Method::Delete => "DELETE",
    }
}

/// How long to keep retrying a request while the box service behind a
/// gateway is restarting (HTTP 502 + `X-Gateway-Error:
/// upstream_unavailable`).
///
/// Only requests that are safe to repeat are retried (reads, and writes
/// that set an absolute state such as USB port enable/disable); see the
/// crate docs for the list. Everything else fails on the first such
/// response with [`crate::Error::BoxServiceUnavailable`].
///
/// The default waits 1s, 2s, 3s, ... between attempts until `budget` (9s)
/// has elapsed since the first attempt, with one last attempt at the
/// deadline: enough to cover the box server's ~4s supervisor respawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Total time, measured from the first attempt, after which no further
    /// attempt is started. `Duration::ZERO` disables retrying.
    pub budget: Duration,
    /// Base backoff step: the n-th retry waits `n * backoff_step`
    /// (clamped to what is left of the budget).
    pub backoff_step: Duration,
}

impl RetryPolicy {
    /// Never retry: the first upstream-unavailable response fails the call.
    pub const fn disabled() -> Self {
        RetryPolicy {
            budget: Duration::ZERO,
            backoff_step: Duration::ZERO,
        }
    }

    /// Delay before retry number `retry` (1-based), given the time already
    /// spent since the first attempt started. `None` means the budget is
    /// spent: give up.
    pub(crate) fn delay_before(&self, retry: u32, elapsed: Duration) -> Option<Duration> {
        let remaining = self.budget.checked_sub(elapsed)?;
        if remaining.is_zero() || self.backoff_step.is_zero() {
            return None;
        }
        Some((self.backoff_step * retry).min(remaining))
    }
}

impl Default for RetryPolicy {
    fn default() -> Self {
        RetryPolicy {
            budget: Duration::from_secs(9),
            backoff_step: Duration::from_secs(1),
        }
    }
}

/// Whether `req` may be sent again after an upstream-unavailable response.
///
/// The gateway answers `upstream_unavailable` when the box service did not
/// accept its connection, so normally the request never ran. But the
/// service can also die *while* handling a request, so only operations
/// whose repetition cannot change the outcome are allowed: reads, and
/// writes that drive the hardware to an absolute state. Relative or
/// one-shot actions (toggle, cycle, recover, bus transfers, moves, flashes,
/// connects) are never retried. Unknown actions default to not retryable.
pub fn is_retry_safe(req: &HttpRequest) -> bool {
    match req.method {
        // Every GET this crate sends is a read (/health, /status,
        // /nets/list, /uart/nets/list, /nets/state, /usb/devices, /lock).
        Method::Get => true,
        // PUT /nets/<name>/safety-limits replaces the whole record.
        Method::Put => req.path.starts_with("/nets/") && req.path.ends_with("/safety-limits"),
        Method::Post => post_is_retry_safe(req),
        // DELETE (admin file removal) is idempotent in principle, but the
        // second attempt would report "not found" for a delete that worked.
        Method::Delete => false,
    }
}

fn post_is_retry_safe(req: &HttpRequest) -> bool {
    let Some(body) = req.body.as_ref() else {
        return false;
    };
    let field = |key: &str| body.get(key).and_then(Value::as_str);
    let action = field("action").unwrap_or("");
    match req.path.as_str() {
        // Not toggle, cycle or recover: each of those changes the outcome
        // if it runs twice.
        "/usb/command" => matches!(action, "enable" | "disable" | "state"),
        // Absolute setpoints and reads. Not clear_ocp/clear_ovp.
        "/supply/command" => matches!(
            action,
            "state"
                | "enable"
                | "disable"
                | "voltage"
                | "current"
                | "ocp"
                | "ovp"
                | "disable_ocp"
                | "disable_ovp"
                | "set_mode"
        ),
        "/battery/command" => matches!(
            action,
            "state"
                | "enable_battery"
                | "disable_battery"
                | "set_soc"
                | "set_voc"
                | "set_volt_full"
                | "set_volt_empty"
                | "set_capacity"
                | "set_current_limit"
                | "set_ocp"
                | "set_ovp"
                | "set_mode"
                | "set_model"
                | "set_esr"
                | "list_models"
                | "export_model"
        ),
        "/net/command" => net_command_is_retry_safe(field("role"), action, body),
        "/wifi/command" => action == "status",
        "/blufi/command" => matches!(action, "status" | "version"),
        "/usb/dfu" => action == "list",
        // /ble/command (scan/connect/...), /lock, /lock/heartbeat, /unlock,
        // and the debug service (flash/erase/reset/...) are not retried.
        // POST /lock looks idempotent for the same user, but its response
        // (`previous_user`) would change meaning on a repeat.
        _ => false,
    }
}

fn net_command_is_retry_safe(role: Option<&str>, action: &str, body: &Value) -> bool {
    match role {
        Some("adc" | "thermocouple") => action == "read",
        Some("dac") => matches!(action, "read" | "set"),
        Some("gpio") => match action {
            "input" | "output_high" | "output_low" => true,
            // `output` carries the level; `toggle` is relative.
            "output" => matches!(
                body.pointer("/params/level").and_then(Value::as_str),
                Some("high" | "low")
            ),
            _ => false,
        },
        // Mode reads/setpoints (`cc`/`cv`/`cr`/`cp`, with or without a
        // value) are absolute.
        Some("eload") => matches!(
            action,
            "state" | "enable" | "disable" | "cc" | "cv" | "cr" | "cp"
        ),
        // Not `set`/`stop` (entering and leaving solar mode).
        Some("solar") => matches!(
            action,
            "irradiance" | "mpp_current" | "mpp_voltage" | "resistance" | "temperature" | "voc"
        ),
        Some("watt-meter") => matches!(action, "power" | "current" | "voltage" | "all"),
        Some("energy-analyzer") => matches!(action, "read_energy" | "read_stats"),
        Some("webcam") => matches!(action, "status" | "url"),
        Some("arm") => matches!(action, "position" | "get_acceleration"),
        // `measure` configures the meter to the same function/range each
        // time. Not `errors`: reading the SCPI error queue drains it.
        Some("dmm") => matches!(action, "measure" | "identify"),
        Some("hsm") => action == "info",
        // Router nets omit the role hint.
        None | Some("router" | "mikrotik") => matches!(
            action,
            "system_info"
                | "interfaces"
                | "wireless_interfaces"
                | "wireless_clients"
                | "dhcp_leases"
        ),
        // SPI/I2C reads clock the bus and can drain device FIFOs; never
        // repeat a bus transaction.
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{self, Timeout};
    use serde_json::json;

    #[test]
    fn header_contract() {
        assert!(is_upstream_unavailable(502, Some("upstream_unavailable")));
        assert!(!is_upstream_unavailable(502, None));
        assert!(!is_upstream_unavailable(502, Some("something_else")));
        assert!(!is_upstream_unavailable(503, Some("upstream_unavailable")));
        assert!(!is_upstream_unavailable(200, Some("upstream_unavailable")));
    }

    #[test]
    fn default_schedule_is_1_2_3_then_clamped_to_the_deadline() {
        let p = RetryPolicy::default();
        let s = Duration::from_secs;
        assert_eq!(p.delay_before(1, s(0)), Some(s(1)));
        assert_eq!(p.delay_before(2, s(1)), Some(s(2)));
        assert_eq!(p.delay_before(3, s(3)), Some(s(3)));
        assert_eq!(p.delay_before(4, s(6)), Some(s(3)));
        assert_eq!(p.delay_before(5, s(9)), None);
        assert_eq!(RetryPolicy::disabled().delay_before(1, s(0)), None);
    }

    #[test]
    fn gateway_message_prefers_json_error() {
        assert_eq!(gateway_message(r#"{"error":"down"}"#), "down");
        assert_eq!(gateway_message("plain text"), "plain text");
    }

    #[test]
    fn allowlist() {
        assert!(is_retry_safe(&wire::usb_command("u", "enable")));
        assert!(is_retry_safe(&wire::usb_command("u", "disable")));
        assert!(is_retry_safe(&wire::usb_command("u", "state")));
        assert!(!is_retry_safe(&wire::usb_command("u", "toggle")));
        assert!(!is_retry_safe(&wire::usb_command("u", "recover")));
        assert!(!is_retry_safe(&wire::usb_cycle("u", None)));
        assert!(is_retry_safe(&wire::get("/nets/list")));
        assert!(is_retry_safe(&wire::lock_status()));
        assert!(!is_retry_safe(&wire::lock_acquire("me", "user", None)));
        assert!(!is_retry_safe(&wire::unlock("me", false)));
        assert!(is_retry_safe(&wire::nets_state()));
        let gpio = |params| wire::net_command("g", "gpio", "output", params, Timeout::Default);
        assert!(is_retry_safe(&gpio(json!({"level": "high"}))));
        assert!(!is_retry_safe(&gpio(json!({"level": "toggle"}))));
        let spi = wire::net_command("s", "spi", "read", json!({}), Timeout::Default);
        assert!(!is_retry_safe(&spi));
        let adc = wire::net_command("a", "adc", "read", json!({}), Timeout::Default);
        assert!(is_retry_safe(&adc));
        let debug = wire::debug_request("/debug/flash", json!({}), Timeout::Default);
        assert!(!is_retry_safe(&debug));
    }
}
