//! Blocking client for the Lager box HTTP API (default feature).

use std::time::Duration;

use serde_json::Value;

use crate::auth::{self, GatewayAuth};
use crate::error::{Error, Result};
use crate::nets::adc::Adc;
use crate::nets::arm::Arm;
use crate::nets::battery::Battery;
use crate::nets::ble::Ble;
use crate::nets::blufi::Blufi;
use crate::nets::dac::Dac;
use crate::nets::debug::DebugNet;
use crate::nets::dfu::Dfu;
use crate::nets::eload::Eload;
use crate::nets::energy::EnergyAnalyzer;
use crate::nets::gpio::Gpio;
use crate::nets::i2c::I2c;
use crate::nets::router::Router;
use crate::nets::scope::Scope;
use crate::nets::solar::Solar;
use crate::nets::spi::Spi;
use crate::nets::supply::Supply;
use crate::nets::thermocouple::Thermocouple;
use crate::nets::usb::UsbPort;
use crate::nets::watt::WattMeter;
use crate::nets::webcam::Webcam;
use crate::nets::wifi::Wifi;
use crate::wire::{
    self, BoxLock, BoxStatus, Health, HttpRequest, Method, NetRecord, NetState, Op, SafetyLimits,
    Timeout, UsbDeviceFilter, UsbDeviceInfo,
};
use crate::BOX_HOST_ENV;

/// A connection to one Lager box, over blocking HTTP.
///
/// Cheap to construct: no network traffic happens until the first call.
/// Net handles borrow the client, so a typical test creates one `LagerBox`
/// and any number of handles from it.
///
/// ```no_run
/// use lager::LagerBox;
///
/// fn main() -> lager::Result<()> {
///     let lager = LagerBox::connect("192.168.1.42")?;
///     let supply = lager.supply("supply1");
///     supply.set_voltage(3.3)?;
///     supply.enable()?;
///     let v = lager.adc("vbat_sense").read()?;
///     assert!((v - 3.3).abs() < 0.1);
///     supply.disable()
/// }
/// ```
pub struct LagerBox {
    base: String,
    debug_base: String,
    agent: ureq::Agent,
    default_timeout: Duration,
    auth: GatewayAuth,
}

/// Builder for [`LagerBox`], for overriding the default timeout, the
/// debug-service URL, and gateway auth.
pub struct LagerBoxBuilder {
    host: String,
    debug_url: Option<String>,
    default_timeout: Duration,
    bearer_token: Option<String>,
}

impl LagerBoxBuilder {
    /// Override the default HTTP timeout for quick commands (10s unless
    /// changed). Long-running actions (watt/energy windows,
    /// `wait_for_level`) still compute their own wider budgets.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.default_timeout = timeout;
        self
    }

    /// Override the debug-service base URL (default: the box host on port
    /// 8765). Use this when the debug service is reached through an SSH
    /// tunnel (e.g. `http://127.0.0.1:8765`). Also settable via the
    /// `LAGER_DEBUG_SERVICE_URL` environment variable.
    pub fn debug_service_url(mut self, url: impl Into<String>) -> Self {
        self.debug_url = Some(url.into());
        self
    }

    /// Attach `Authorization: Bearer <token>` to every request, for boxes
    /// behind an authenticating gateway. Also settable via the
    /// `LAGER_GATEWAY_TOKEN` environment variable.
    ///
    /// Without this, the crate reuses the Lager CLI's session
    /// (`lager login <auth_url>`, stored in `~/.lager_gateway_auth`)
    /// automatically when a gateway asks for auth, including transparent
    /// refresh of expired access tokens. Plain (ungated) boxes are
    /// unaffected either way.
    pub fn bearer_token(mut self, token: impl Into<String>) -> Self {
        self.bearer_token = Some(token.into());
        self
    }

    /// Build the client.
    pub fn build(self) -> Result<LagerBox> {
        let base = wire::base_url(&self.host)?;
        let debug_base = match self.debug_url {
            Some(url) => wire::base_url_with_port(&url, wire::DEBUG_SERVICE_PORT)?,
            None => wire::service_base(&base, wire::DEBUG_SERVICE_PORT),
        };
        let auth = GatewayAuth::new(&base, self.bearer_token);
        Ok(LagerBox {
            base,
            debug_base,
            agent: ureq::AgentBuilder::new().build(),
            default_timeout: self.default_timeout,
            auth,
        })
    }
}

impl LagerBox {
    /// Connect to a box by host name, IP, `host:port`, or full URL.
    /// The port defaults to 9000 (the box HTTP server).
    pub fn connect(host: impl Into<String>) -> Result<Self> {
        Self::builder(host).build()
    }

    /// Connect to the box named by the `LAGER_BOX_HOST` environment
    /// variable.
    pub fn from_env() -> Result<Self> {
        let host = std::env::var(BOX_HOST_ENV)
            .map_err(|_| Error::Config(format!("{BOX_HOST_ENV} is not set")))?;
        Self::connect(host)
    }

    /// Start building a client with non-default settings.
    pub fn builder(host: impl Into<String>) -> LagerBoxBuilder {
        LagerBoxBuilder {
            host: host.into(),
            debug_url: std::env::var(crate::DEBUG_SERVICE_URL_ENV).ok(),
            default_timeout: wire::DEFAULT_TIMEOUT,
            bearer_token: None,
        }
    }

    /// The base URL this client talks to, e.g. `http://192.168.1.42:9000`.
    pub fn base_url(&self) -> &str {
        &self.base
    }

    // -- transport ---------------------------------------------------------

    /// Send one request against the port-9000 server.
    pub(crate) fn execute(&self, req: &HttpRequest) -> Result<(u16, Value)> {
        self.execute_at(&self.base, req)
    }

    /// Send one request against the debug service (port 8765).
    pub(crate) fn execute_debug(&self, req: &HttpRequest) -> Result<(u16, Value)> {
        self.execute_at(&self.debug_base, req)
    }

    /// Send one request against an arbitrary base URL and return
    /// `(status, parsed JSON body)`. Attaches gateway auth when known, and
    /// handles a gateway denial by resolving credentials (CLI session
    /// store, with transparent refresh) and retrying once.
    fn execute_at(&self, base: &str, req: &HttpRequest) -> Result<(u16, Value)> {
        let token = self.current_token();
        let (status, gateway, resp_body) = self.send_once(base, req, token.as_deref())?;

        let Some(auth_url) = gateway else {
            return Ok((status, resp_body));
        };
        // Gateway denial: learn the box→auth-server mapping (like the CLI),
        // then retry once with a credential the gateway has not just seen.
        self.auth.learn_auth_server(&auth_url);
        if status == 401 && !self.auth.has_static_token() {
            if let Some(fresh) = self
                .auth
                .resolve_token_blocking(&auth_url, token.as_deref())
            {
                let (status, gateway, resp_body) =
                    self.send_once(base, req, Some(&fresh))?;
                let Some(auth_url) = gateway else {
                    return Ok((status, resp_body));
                };
                return Err(auth::denial_error(
                    status,
                    self.auth.box_host(),
                    &auth_url,
                    true,
                ));
            }
        }
        Err(auth::denial_error(
            status,
            self.auth.box_host(),
            &auth_url,
            token.is_some(),
        ))
    }

    /// Token to attach right now: builder/env token, cached session token,
    /// or a store lookup when the box is already known to be gated.
    /// `pub(crate)`: the Socket.IO session openers (uart, rtt) attach it to
    /// their handshake too.
    pub(crate) fn current_token(&self) -> Option<String> {
        if let Some(token) = self.auth.cached_token() {
            return Some(token);
        }
        if self.auth.wants_store_token() {
            let auth_url = self.auth.auth_url()?;
            return self.auth.resolve_token_blocking(&auth_url, None);
        }
        None
    }

    /// One HTTP round-trip. Returns `(status, gateway_denial_auth_url,
    /// body)`; the auth URL is `Some` only for a gateway denial (401/403/
    /// 503 carrying the discovery header).
    fn send_once(
        &self,
        base: &str,
        req: &HttpRequest,
        token: Option<&str>,
    ) -> Result<(u16, Option<String>, Value)> {
        let url = format!("{}{}", base, req.path);
        let mut r = match req.method {
            Method::Get => self.agent.request("GET", &url),
            Method::Post => self.agent.request("POST", &url),
            Method::Put => self.agent.request("PUT", &url),
        };
        match req.timeout {
            Timeout::Default => r = r.timeout(self.default_timeout),
            Timeout::After(d) => r = r.timeout(d),
            Timeout::Unbounded => {}
        }
        if let Some(token) = token {
            r = r.set("Authorization", &format!("Bearer {token}"));
        }
        let outcome = match (&req.method, &req.body) {
            (Method::Post | Method::Put, Some(body)) => r.send_json(body.clone()),
            _ => r.call(),
        };
        let resp = match outcome {
            Ok(resp) => resp,
            // ureq reports 4xx/5xx as Err(Status); the box still sends a
            // JSON error body we need to surface.
            Err(ureq::Error::Status(_, resp)) => resp,
            Err(ureq::Error::Transport(t)) => {
                let msg = t.to_string();
                return if msg.to_ascii_lowercase().contains("timed out")
                    || msg.to_ascii_lowercase().contains("timeout")
                {
                    Err(Error::Timeout(msg))
                } else {
                    Err(Error::Connection(msg))
                };
            }
        };
        let status = resp.status();
        let gateway = if auth::is_denial(status) {
            resp.header(auth::DISCOVERY_HEADER).map(str::to_string)
        } else {
            None
        };
        let body: Value = match resp.into_json() {
            Ok(body) => body,
            Err(e) if status < 400 => {
                return Err(Error::Decode(format!("non-JSON response: {e}")))
            }
            // Error responses (incl. gateway denials) may have no JSON body.
            Err(_) => Value::Null,
        };
        if gateway.is_none() && status >= 400 && body.is_null() {
            return Err(Error::Box {
                status,
                message: format!("HTTP {status} (non-JSON body)"),
            });
        }
        Ok((status, gateway, body))
    }

    /// Execute one typed operation against a command endpoint.
    pub(crate) fn run<T>(&self, op: Op<T>) -> Result<T> {
        let (status, body) = self.execute(&op.req)?;
        let resp = wire::parse_command(status, body)?;
        (op.parse)(resp)
    }

    /// Open a streaming response body against the debug service (used by
    /// RTT). Returns the raw byte reader.
    pub(crate) fn stream_debug(
        &self,
        req: &HttpRequest,
    ) -> Result<Box<dyn std::io::Read + Send + Sync>> {
        let token = self.current_token();
        match self.stream_debug_once(req, token.as_deref()) {
            // Gateway denial (only a 401 maps to AuthRequired): resolve
            // credentials from the CLI session store — avoiding the token
            // the gateway just rejected — and retry once, like execute_at.
            Err(Error::AuthRequired {
                box_host,
                auth_url,
                message,
            }) if !self.auth.has_static_token() => {
                match self
                    .auth
                    .resolve_token_blocking(&auth_url, token.as_deref())
                {
                    Some(fresh) => self.stream_debug_once(req, Some(&fresh)),
                    None => Err(Error::AuthRequired {
                        box_host,
                        auth_url,
                        message,
                    }),
                }
            }
            other => other,
        }
    }

    fn stream_debug_once(
        &self,
        req: &HttpRequest,
        token: Option<&str>,
    ) -> Result<Box<dyn std::io::Read + Send + Sync>> {
        let url = format!("{}{}", self.debug_base, req.path);
        let mut r = self.agent.request("POST", &url);
        match req.timeout {
            Timeout::Default => r = r.timeout(self.default_timeout),
            Timeout::After(d) => r = r.timeout(d),
            Timeout::Unbounded => {}
        }
        if let Some(token) = token {
            r = r.set("Authorization", &format!("Bearer {token}"));
        }
        let outcome = match &req.body {
            Some(body) => r.send_json(body.clone()),
            None => r.call(),
        };
        match outcome {
            Ok(resp) => Ok(resp.into_reader()),
            Err(ureq::Error::Status(status, resp)) => {
                if auth::is_denial(status) {
                    if let Some(auth_url) = resp.header(auth::DISCOVERY_HEADER) {
                        let auth_url = auth_url.to_string();
                        self.auth.learn_auth_server(&auth_url);
                        return Err(auth::denial_error(
                            status,
                            self.auth.box_host(),
                            &auth_url,
                            token.is_some(),
                        ));
                    }
                }
                let body: Value = resp.into_json().unwrap_or(Value::Null);
                Err(wire::parse_debug(status, body).unwrap_err())
            }
            Err(ureq::Error::Transport(t)) => Err(Error::Connection(t.to_string())),
        }
    }

    /// Resolve a debug net's full saved record (needed by the debug service,
    /// which reads probe fields out of the record).
    pub(crate) fn debug_net_record(&self, name: &str) -> Result<Value> {
        let records = self.nets_raw()?;
        crate::nets::debug::find_debug_record(records, name)
    }

    /// Raw saved-net records (untyped), with the same `/nets/list` ->
    /// `/uart/nets/list` fallback as [`LagerBox::nets`].
    fn nets_raw(&self) -> Result<Vec<Value>> {
        let body = match self.get_json("/nets/list") {
            Ok(body) => body,
            Err(primary) => self.get_json("/uart/nets/list").map_err(|_| primary)?,
        };
        Ok(wire::nets_list_values(body))
    }

    fn get_json(&self, path: &str) -> Result<Value> {
        let (status, body) = self.execute(&wire::get(path))?;
        if status != 200 {
            return Err(Error::Box {
                status,
                message: body
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("request failed")
                    .to_string(),
            });
        }
        Ok(body)
    }

    // -- box-level queries --------------------------------------------------

    /// List every net configured on the box (full saved records).
    ///
    /// Falls back to the older `/uart/nets/list` shape for box images that
    /// predate `/nets/list`, like the Lager CLI does.
    pub fn nets(&self) -> Result<Vec<NetRecord>> {
        match self.get_json("/nets/list") {
            Ok(body) => wire::nets_from_body(body),
            Err(primary) => match self.get_json("/uart/nets/list") {
                Ok(body) => wire::nets_from_body(body),
                Err(_) => Err(primary),
            },
        }
    }

    /// Check that the box HTTP server is up.
    pub fn health(&self) -> Result<Health> {
        let body = self.get_json("/health")?;
        serde_json::from_value(body).map_err(Into::into)
    }

    /// Box status: version, configured nets, and endpoint capabilities.
    pub fn status(&self) -> Result<BoxStatus> {
        let body = self.get_json("/status")?;
        serde_json::from_value(body).map_err(Into::into)
    }

    /// Enumerate USB devices on the box's bus from sysfs (lsusb-like).
    ///
    /// A few milliseconds per call with no exclusive device access, so it
    /// is safe to poll frequently — e.g. reading the DUT's iSerial to see
    /// what it re-enumerated as after a hub power-cycle or DFU detach.
    ///
    /// Requires box software >= 0.33.0; older boxes fail with
    /// [`Error::UnsupportedByBox`].
    pub fn usb_devices(&self) -> Result<Vec<UsbDeviceInfo>> {
        self.usb_devices_matching(&UsbDeviceFilter::default())
    }

    /// Like [`LagerBox::usb_devices`], with box-side vid/pid/serial
    /// filters.
    pub fn usb_devices_matching(&self, filter: &UsbDeviceFilter) -> Result<Vec<UsbDeviceInfo>> {
        match self.execute(&wire::usb_devices(filter)) {
            Ok((status, body)) => wire::parse_usb_devices(status, body),
            Err(e) => Err(wire::map_route_missing(e, wire::usb_devices_unsupported)),
        }
    }

    // -- box lock / reservation ----------------------------------------------

    fn lock_call(&self, req: &HttpRequest) -> Result<BoxLock> {
        match self.execute(req) {
            Ok((status, body)) => wire::parse_lock(status, body),
            Err(e) => Err(wire::map_route_missing(e, wire::lock_unsupported)),
        }
    }

    /// Current box lock state (`GET /lock`); `locked: false` when free.
    pub fn lock_status(&self) -> Result<BoxLock> {
        self.lock_call(&wire::lock_status())
    }

    /// Claim the box for `user` (an eternal `holder_type: "user"` lock,
    /// exactly like `lager boxes lock`). Re-acquiring your own lock
    /// refreshes it; a box held by someone else fails with
    /// [`Error::Box`] (HTTP 409) naming the holder.
    pub fn lock(&self, user: &str) -> Result<BoxLock> {
        self.lock_call(&wire::lock_acquire(user, "user", None))
    }

    /// Claim the box with an explicit holder type and TTL.
    /// `ttl_seconds: None` means the lock never auto-expires; with a TTL,
    /// keep the lock alive via [`LagerBox::lock_heartbeat`].
    pub fn lock_with(
        &self,
        user: &str,
        holder_type: &str,
        ttl_seconds: Option<u64>,
    ) -> Result<BoxLock> {
        self.lock_call(&wire::lock_acquire(user, holder_type, ttl_seconds))
    }

    /// Refresh a TTL lock's heartbeat. Fails with [`Error::Box`] when the
    /// box is not locked (HTTP 404) or held by someone else (HTTP 403).
    pub fn lock_heartbeat(&self, user: &str) -> Result<BoxLock> {
        self.lock_call(&wire::lock_heartbeat(user))
    }

    /// Release `user`'s box lock. Releasing an already-unlocked box
    /// succeeds; a box held by someone else fails with [`Error::Box`]
    /// (HTTP 403).
    pub fn unlock(&self, user: &str) -> Result<()> {
        self.lock_call(&wire::unlock(user, false)).map(|_| ())
    }

    /// Release the box lock even when held by another user
    /// (`lager boxes unlock --force`).
    pub fn unlock_force(&self, user: &str) -> Result<()> {
        self.lock_call(&wire::unlock(user, true)).map(|_| ())
    }

    /// Claim the box for `user` and release the claim when the returned
    /// guard drops (best-effort; call [`BoxLockGuard::unlock`] to surface
    /// release errors).
    pub fn lock_guard(&self, user: impl Into<String>) -> Result<BoxLockGuard<'_>> {
        let user = user.into();
        self.lock(&user)?;
        Ok(BoxLockGuard { client: self, user, released: false })
    }

    // -- per-net safety limits ------------------------------------------------

    /// Set the safety limits on a saved net (`PUT /nets/<name>/safety-limits`,
    /// box >= 0.35.0). Returns the limits the box applied.
    ///
    /// The PUT **replaces** the net's whole limits record: fields left `None`
    /// in `limits` are removed from the net, not preserved. Read the current
    /// limits first ([`LagerBox::safety_limits`]) if you mean to change one
    /// ceiling and keep the rest. An all-`None` `limits` clears the record,
    /// same as [`LagerBox::clear_safety_limits`].
    ///
    /// The ceilings are enforced by the box's hardware service, out of reach
    /// of test scripts; a setpoint (or inline `ovp=`/`ocp=` trip) above a
    /// ceiling is refused before it touches the instrument. Older boxes fail
    /// with [`Error::UnsupportedByBox`]; validation refusals (`max_power`,
    /// non-positive ceilings) and an unknown net come back as [`Error::Box`].
    pub fn set_safety_limits(
        &self,
        name: &str,
        limits: &SafetyLimits,
    ) -> Result<Option<SafetyLimits>> {
        match self.execute(&wire::safety_limits_set(name, limits)) {
            Ok((status, body)) => wire::parse_safety_limits(status, body),
            Err(e) => Err(wire::map_route_missing(e, wire::safety_limits_unsupported)),
        }
    }

    /// Remove a net's safety limits, returning it to unrestricted.
    pub fn clear_safety_limits(&self, name: &str) -> Result<()> {
        self.set_safety_limits(name, &SafetyLimits::default())
            .map(|_| ())
    }

    /// Brief live state for every saved net (`GET /nets/state`,
    /// box >= 0.34.0): what `lager nets state` shows. One entry per saved
    /// net; an instrument that is slow, wedged or absent yields
    /// `state: None` with a reason rather than failing the request, so this
    /// is safe to call as a bench health check. Older boxes fail with
    /// [`Error::UnsupportedByBox`].
    pub fn nets_state(&self) -> Result<Vec<NetState>> {
        match self.execute(&wire::nets_state()) {
            Ok((status, body)) => wire::parse_nets_state(status, body),
            Err(e) => Err(wire::map_route_missing(e, wire::nets_state_unsupported)),
        }
    }

    /// Read the safety limits configured on a saved net, via `/nets/list`.
    /// `Ok(None)` means the net exists and is unrestricted; a missing net is
    /// an [`Error::Box`] with status 404.
    pub fn safety_limits(&self, name: &str) -> Result<Option<SafetyLimits>> {
        let nets = self.nets()?;
        nets.iter()
            .find(|rec| rec.name == name)
            .map(|rec| rec.safety_limits)
            .ok_or_else(|| Error::Box {
                status: 404,
                message: format!("no saved net named '{name}' on this box"),
            })
    }

    // -- net handle constructors ---------------------------------------------

    /// Handle for a power-supply net.
    pub fn supply(&self, name: impl Into<String>) -> Supply<'_> {
        Supply { client: self, name: name.into() }
    }

    /// Handle for a battery-simulator net.
    pub fn battery(&self, name: impl Into<String>) -> Battery<'_> {
        Battery { client: self, name: name.into() }
    }

    /// Handle for an electronic-load net.
    pub fn eload(&self, name: impl Into<String>) -> Eload<'_> {
        Eload { client: self, name: name.into() }
    }

    /// Handle for a solar-simulator net (EA PSB photovoltaic mode).
    pub fn solar(&self, name: impl Into<String>) -> Solar<'_> {
        Solar { client: self, name: name.into() }
    }

    /// Handle for a GPIO net.
    pub fn gpio(&self, name: impl Into<String>) -> Gpio<'_> {
        Gpio { client: self, name: name.into() }
    }

    /// Handle for an ADC net.
    pub fn adc(&self, name: impl Into<String>) -> Adc<'_> {
        Adc { client: self, name: name.into() }
    }

    /// Handle for a DAC net.
    pub fn dac(&self, name: impl Into<String>) -> Dac<'_> {
        Dac { client: self, name: name.into() }
    }

    /// Handle for a thermocouple net.
    pub fn thermocouple(&self, name: impl Into<String>) -> Thermocouple<'_> {
        Thermocouple { client: self, name: name.into() }
    }

    /// Handle for a watt-meter net.
    pub fn watt_meter(&self, name: impl Into<String>) -> WattMeter<'_> {
        WattMeter { client: self, name: name.into() }
    }

    /// Handle for an energy-analyzer net.
    pub fn energy_analyzer(&self, name: impl Into<String>) -> EnergyAnalyzer<'_> {
        EnergyAnalyzer { client: self, name: name.into() }
    }

    /// Handle for an SPI bus net.
    pub fn spi(&self, name: impl Into<String>) -> Spi<'_> {
        Spi { client: self, name: name.into() }
    }

    /// Handle for an I2C bus net.
    pub fn i2c(&self, name: impl Into<String>) -> I2c<'_> {
        I2c { client: self, name: name.into() }
    }

    /// Handle for a USB hub port net.
    pub fn usb(&self, name: impl Into<String>) -> UsbPort<'_> {
        UsbPort { client: self, name: name.into() }
    }

    /// Handle for a robot-arm net (Rotrics Dexarm).
    pub fn arm(&self, name: impl Into<String>) -> Arm<'_> {
        Arm { client: self, name: name.into() }
    }

    /// Handle for a webcam net (MJPEG streaming).
    pub fn webcam(&self, name: impl Into<String>) -> Webcam<'_> {
        Webcam { client: self, name: name.into() }
    }

    /// Handle for a router net (MikroTik RouterOS).
    pub fn router(&self, name: impl Into<String>) -> Router<'_> {
        Router { client: self, name: name.into() }
    }

    /// Handle for the box's BLE adapter (box-level, not a saved net).
    pub fn ble(&self) -> Ble<'_> {
        Ble { client: self }
    }

    /// Handle for the box's WiFi interface (box-level, not a saved net).
    pub fn wifi(&self) -> Wifi<'_> {
        Wifi { client: self }
    }

    /// Handle for BluFi (ESP32 WiFi provisioning over BLE; box-level).
    pub fn blufi(&self) -> Blufi<'_> {
        Blufi { client: self }
    }

    /// Handle for box-side DFU via dfu-util (box-level, not a saved net).
    pub fn dfu(&self) -> Dfu<'_> {
        Dfu { client: self }
    }

    /// Handle for a debug-probe net (flash/erase/reset/read_memory/RTT).
    /// Talks to the box debug service on port 8765.
    pub fn debug(&self, name: impl Into<String>) -> DebugNet<'_> {
        DebugNet { client: self, name: name.into(), record: Default::default() }
    }

    /// Handle for an oscilloscope net. **Stub:** see [`Scope`].
    pub fn scope(&self, name: impl Into<String>) -> Scope {
        Scope::new(name)
    }

    /// Open a streaming UART session on a UART net.
    ///
    /// Connects a Socket.IO session to the box's `/uart` namespace and
    /// starts streaming. Only one session per net (box-enforced).
    #[cfg(feature = "uart")]
    pub fn uart(&self, name: impl Into<String>) -> Result<crate::nets::uart::Uart> {
        crate::nets::uart::Uart::open(&self.base, name.into(), self.current_token())
    }
}

/// RAII box-lock claim from [`LagerBox::lock_guard`]: releases the lock on
/// drop (best-effort — a drop cannot surface errors, so tests that must
/// know the release worked should call [`BoxLockGuard::unlock`]).
pub struct BoxLockGuard<'a> {
    client: &'a LagerBox,
    user: String,
    released: bool,
}

impl BoxLockGuard<'_> {
    /// The user holding this claim.
    pub fn user(&self) -> &str {
        &self.user
    }

    /// Release the lock now, surfacing any error.
    pub fn unlock(mut self) -> Result<()> {
        self.released = true;
        self.client.unlock(&self.user)
    }
}

impl Drop for BoxLockGuard<'_> {
    fn drop(&mut self) {
        if !self.released {
            let _ = self.client.unlock(&self.user);
        }
    }
}
