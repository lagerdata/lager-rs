//! Async client for the Lager box HTTP API (feature `async`).
//!
//! Runs the exact same request builders and response parsers as the
//! blocking client — only the transport differs.

use std::time::Duration;

use serde_json::Value;

use crate::auth::{self, GatewayAuth};
use crate::error::{Error, Result};
use crate::nets::adc::AsyncAdc;
use crate::nets::arm::AsyncArm;
use crate::nets::battery::AsyncBattery;
use crate::nets::ble::AsyncBle;
use crate::nets::blufi::AsyncBlufi;
use crate::nets::dac::AsyncDac;
use crate::nets::debug::AsyncDebugNet;
use crate::nets::dfu::AsyncDfu;
use crate::nets::eload::AsyncEload;
use crate::nets::energy::AsyncEnergyAnalyzer;
use crate::nets::gpio::AsyncGpio;
use crate::nets::i2c::AsyncI2c;
use crate::nets::router::AsyncRouter;
use crate::nets::scope::Scope;
use crate::nets::solar::AsyncSolar;
use crate::nets::spi::AsyncSpi;
use crate::nets::supply::AsyncSupply;
use crate::nets::thermocouple::AsyncThermocouple;
use crate::nets::usb::AsyncUsbPort;
use crate::nets::watt::AsyncWattMeter;
use crate::nets::webcam::AsyncWebcam;
use crate::nets::wifi::AsyncWifi;
use crate::wire::{
    self, BoxLock, BoxStatus, Health, HttpRequest, Method, NetRecord, Op, Timeout,
    UsbDeviceFilter, UsbDeviceInfo,
};

/// A connection to one Lager box, over async HTTP (reqwest/tokio).
///
/// ```no_run
/// use lager::AsyncLagerBox;
///
/// #[tokio::main]
/// async fn main() -> lager::Result<()> {
///     let lager = AsyncLagerBox::connect("192.168.1.42")?;
///     let supply = lager.supply("supply1");
///     supply.set_voltage(3.3).await?;
///     supply.enable().await?;
///     let v = lager.adc("vbat_sense").read().await?;
///     assert!((v - 3.3).abs() < 0.1);
///     supply.disable().await
/// }
/// ```
pub struct AsyncLagerBox {
    base: String,
    debug_base: String,
    http: reqwest::Client,
    default_timeout: Duration,
    auth: GatewayAuth,
}

/// Builder for [`AsyncLagerBox`], for overriding the default timeout, the
/// debug-service URL, and gateway auth.
pub struct AsyncLagerBoxBuilder {
    host: String,
    debug_url: Option<String>,
    default_timeout: Duration,
    bearer_token: Option<String>,
}

impl AsyncLagerBoxBuilder {
    /// Override the default HTTP timeout for quick commands (10s unless
    /// changed). Long-running actions still compute their own wider budgets.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.default_timeout = timeout;
        self
    }

    /// Override the debug-service base URL (default: the box host on port
    /// 8765). Also settable via `LAGER_DEBUG_SERVICE_URL`.
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
    pub fn build(self) -> Result<AsyncLagerBox> {
        let base = wire::base_url(&self.host)?;
        let debug_base = match self.debug_url {
            Some(url) => wire::base_url_with_port(&url, wire::DEBUG_SERVICE_PORT)?,
            None => wire::service_base(&base, wire::DEBUG_SERVICE_PORT),
        };
        let auth = GatewayAuth::new(&base, self.bearer_token);
        Ok(AsyncLagerBox {
            base,
            debug_base,
            http: reqwest::Client::new(),
            default_timeout: self.default_timeout,
            auth,
        })
    }
}

impl AsyncLagerBox {
    /// Connect to a box by host name, IP, `host:port`, or full URL.
    /// The port defaults to 9000 (the box HTTP server).
    pub fn connect(host: impl Into<String>) -> Result<Self> {
        Self::builder(host).build()
    }

    /// Connect to the box named by the `LAGER_BOX_HOST` environment
    /// variable.
    pub fn from_env() -> Result<Self> {
        let host = std::env::var(crate::BOX_HOST_ENV)
            .map_err(|_| Error::Config(format!("{} is not set", crate::BOX_HOST_ENV)))?;
        Self::connect(host)
    }

    /// Start building a client with non-default settings.
    pub fn builder(host: impl Into<String>) -> AsyncLagerBoxBuilder {
        AsyncLagerBoxBuilder {
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
    pub(crate) async fn execute(&self, req: &HttpRequest) -> Result<(u16, Value)> {
        self.execute_at(&self.base, req).await
    }

    /// Send one request against the debug service (port 8765).
    pub(crate) async fn execute_debug(&self, req: &HttpRequest) -> Result<(u16, Value)> {
        self.execute_at(&self.debug_base, req).await
    }

    /// Send one request against an arbitrary base URL. Attaches gateway
    /// auth when known, and handles a gateway denial by resolving
    /// credentials (CLI session store, with transparent refresh) and
    /// retrying once. Mirrors the blocking client exactly.
    async fn execute_at(&self, base: &str, req: &HttpRequest) -> Result<(u16, Value)> {
        let token = self.current_token().await;
        let (status, gateway, resp_body) = self.send_once(base, req, token.as_deref()).await?;

        let Some(auth_url) = gateway else {
            return Ok((status, resp_body));
        };
        // Gateway denial: learn the box→auth-server mapping (like the CLI),
        // then retry once with a credential the gateway has not just seen.
        self.auth.learn_auth_server(&auth_url);
        if status == 401 && !self.auth.has_static_token() {
            if let Some(fresh) = self
                .auth
                .resolve_token_async(&auth_url, token.as_deref())
                .await
            {
                let (status, gateway, resp_body) =
                    self.send_once(base, req, Some(&fresh)).await?;
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
    async fn current_token(&self) -> Option<String> {
        if let Some(token) = self.auth.cached_token() {
            return Some(token);
        }
        if self.auth.wants_store_token() {
            let auth_url = self.auth.auth_url()?;
            return self.auth.resolve_token_async(&auth_url, None).await;
        }
        None
    }

    /// One HTTP round-trip. Returns `(status, gateway_denial_auth_url,
    /// body)`; the auth URL is `Some` only for a gateway denial (401/403/
    /// 503 carrying the discovery header).
    async fn send_once(
        &self,
        base: &str,
        req: &HttpRequest,
        token: Option<&str>,
    ) -> Result<(u16, Option<String>, Value)> {
        let url = format!("{}{}", base, req.path);
        let mut r = match req.method {
            Method::Get => self.http.get(&url),
            Method::Post => self.http.post(&url),
        };
        match req.timeout {
            Timeout::Default => r = r.timeout(self.default_timeout),
            Timeout::After(d) => r = r.timeout(d),
            Timeout::Unbounded => {}
        }
        if let Some(token) = token {
            r = r.header("Authorization", format!("Bearer {token}"));
        }
        if let Some(body) = &req.body {
            r = r.json(body);
        }
        let resp = r.send().await.map_err(|e| {
            if e.is_timeout() {
                Error::Timeout(e.to_string())
            } else {
                Error::Connection(e.to_string())
            }
        })?;
        let status = resp.status().as_u16();
        let gateway = if auth::is_denial(status) {
            resp.headers()
                .get(auth::DISCOVERY_HEADER)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        } else {
            None
        };
        let body: Value = match resp.json().await {
            Ok(body) => body,
            Err(e) if e.is_timeout() => return Err(Error::Timeout(e.to_string())),
            Err(e) if status < 400 => {
                return Err(Error::Decode(format!("non-JSON response: {e}")))
            }
            // Error responses (incl. gateway denials) may have no JSON body.
            Err(_) => Value::Null,
        };
        Ok((status, gateway, body))
    }

    /// Execute one typed operation against a command endpoint.
    pub(crate) async fn run<T>(&self, op: Op<T>) -> Result<T> {
        let (status, body) = self.execute(&op.req).await?;
        let resp = wire::parse_command(status, body)?;
        (op.parse)(resp)
    }

    /// Resolve a debug net's full saved record for the debug service.
    pub(crate) async fn debug_net_record(&self, name: &str) -> Result<Value> {
        let records = self.nets_raw().await?;
        crate::nets::debug::find_debug_record(records, name)
    }

    /// Raw saved-net records (untyped), with the `/nets/list` ->
    /// `/uart/nets/list` fallback.
    async fn nets_raw(&self) -> Result<Vec<Value>> {
        let body = match self.get_json("/nets/list").await {
            Ok(body) => body,
            Err(primary) => self.get_json("/uart/nets/list").await.map_err(|_| primary)?,
        };
        Ok(wire::nets_list_values(body))
    }

    async fn get_json(&self, path: &str) -> Result<Value> {
        let (status, body) = self.execute(&wire::get(path)).await?;
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
    pub async fn nets(&self) -> Result<Vec<NetRecord>> {
        match self.get_json("/nets/list").await {
            Ok(body) => wire::nets_from_body(body),
            Err(primary) => match self.get_json("/uart/nets/list").await {
                Ok(body) => wire::nets_from_body(body),
                Err(_) => Err(primary),
            },
        }
    }

    /// Check that the box HTTP server is up.
    pub async fn health(&self) -> Result<Health> {
        let body = self.get_json("/health").await?;
        serde_json::from_value(body).map_err(Into::into)
    }

    /// Box status: version, configured nets, and endpoint capabilities.
    pub async fn status(&self) -> Result<BoxStatus> {
        let body = self.get_json("/status").await?;
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
    pub async fn usb_devices(&self) -> Result<Vec<UsbDeviceInfo>> {
        self.usb_devices_matching(&UsbDeviceFilter::default()).await
    }

    /// Like [`AsyncLagerBox::usb_devices`], with box-side vid/pid/serial
    /// filters.
    pub async fn usb_devices_matching(
        &self,
        filter: &UsbDeviceFilter,
    ) -> Result<Vec<UsbDeviceInfo>> {
        match self.execute(&wire::usb_devices(filter)).await {
            Ok((status, body)) => wire::parse_usb_devices(status, body),
            Err(e) => Err(wire::map_route_missing(e, wire::usb_devices_unsupported)),
        }
    }

    // -- box lock / reservation ----------------------------------------------

    async fn lock_call(&self, req: &HttpRequest) -> Result<BoxLock> {
        match self.execute(req).await {
            Ok((status, body)) => wire::parse_lock(status, body),
            Err(e) => Err(wire::map_route_missing(e, wire::lock_unsupported)),
        }
    }

    /// Current box lock state (`GET /lock`); `locked: false` when free.
    pub async fn lock_status(&self) -> Result<BoxLock> {
        self.lock_call(&wire::lock_status()).await
    }

    /// Claim the box for `user` (an eternal `holder_type: "user"` lock,
    /// exactly like `lager boxes lock`). Re-acquiring your own lock
    /// refreshes it; a box held by someone else fails with
    /// [`Error::Box`] (HTTP 409) naming the holder.
    pub async fn lock(&self, user: &str) -> Result<BoxLock> {
        self.lock_call(&wire::lock_acquire(user, "user", None)).await
    }

    /// Claim the box with an explicit holder type and TTL.
    /// `ttl_seconds: None` means the lock never auto-expires; with a TTL,
    /// keep the lock alive via [`AsyncLagerBox::lock_heartbeat`].
    pub async fn lock_with(
        &self,
        user: &str,
        holder_type: &str,
        ttl_seconds: Option<u64>,
    ) -> Result<BoxLock> {
        self.lock_call(&wire::lock_acquire(user, holder_type, ttl_seconds))
            .await
    }

    /// Refresh a TTL lock's heartbeat. Fails with [`Error::Box`] when the
    /// box is not locked (HTTP 404) or held by someone else (HTTP 403).
    pub async fn lock_heartbeat(&self, user: &str) -> Result<BoxLock> {
        self.lock_call(&wire::lock_heartbeat(user)).await
    }

    /// Release `user`'s box lock. Releasing an already-unlocked box
    /// succeeds; a box held by someone else fails with [`Error::Box`]
    /// (HTTP 403).
    pub async fn unlock(&self, user: &str) -> Result<()> {
        self.lock_call(&wire::unlock(user, false)).await.map(|_| ())
    }

    /// Release the box lock even when held by another user
    /// (`lager boxes unlock --force`).
    pub async fn unlock_force(&self, user: &str) -> Result<()> {
        self.lock_call(&wire::unlock(user, true)).await.map(|_| ())
    }

    // -- net handle constructors ---------------------------------------------

    /// Handle for a power-supply net.
    pub fn supply(&self, name: impl Into<String>) -> AsyncSupply<'_> {
        AsyncSupply { client: self, name: name.into() }
    }

    /// Handle for a battery-simulator net.
    pub fn battery(&self, name: impl Into<String>) -> AsyncBattery<'_> {
        AsyncBattery { client: self, name: name.into() }
    }

    /// Handle for an electronic-load net.
    pub fn eload(&self, name: impl Into<String>) -> AsyncEload<'_> {
        AsyncEload { client: self, name: name.into() }
    }

    /// Handle for a solar-simulator net (EA PSB photovoltaic mode).
    pub fn solar(&self, name: impl Into<String>) -> AsyncSolar<'_> {
        AsyncSolar { client: self, name: name.into() }
    }

    /// Handle for a GPIO net.
    pub fn gpio(&self, name: impl Into<String>) -> AsyncGpio<'_> {
        AsyncGpio { client: self, name: name.into() }
    }

    /// Handle for an ADC net.
    pub fn adc(&self, name: impl Into<String>) -> AsyncAdc<'_> {
        AsyncAdc { client: self, name: name.into() }
    }

    /// Handle for a DAC net.
    pub fn dac(&self, name: impl Into<String>) -> AsyncDac<'_> {
        AsyncDac { client: self, name: name.into() }
    }

    /// Handle for a thermocouple net.
    pub fn thermocouple(&self, name: impl Into<String>) -> AsyncThermocouple<'_> {
        AsyncThermocouple { client: self, name: name.into() }
    }

    /// Handle for a watt-meter net.
    pub fn watt_meter(&self, name: impl Into<String>) -> AsyncWattMeter<'_> {
        AsyncWattMeter { client: self, name: name.into() }
    }

    /// Handle for an energy-analyzer net.
    pub fn energy_analyzer(&self, name: impl Into<String>) -> AsyncEnergyAnalyzer<'_> {
        AsyncEnergyAnalyzer { client: self, name: name.into() }
    }

    /// Handle for an SPI bus net.
    pub fn spi(&self, name: impl Into<String>) -> AsyncSpi<'_> {
        AsyncSpi { client: self, name: name.into() }
    }

    /// Handle for an I2C bus net.
    pub fn i2c(&self, name: impl Into<String>) -> AsyncI2c<'_> {
        AsyncI2c { client: self, name: name.into() }
    }

    /// Handle for a USB hub port net.
    pub fn usb(&self, name: impl Into<String>) -> AsyncUsbPort<'_> {
        AsyncUsbPort { client: self, name: name.into() }
    }

    /// Handle for a robot-arm net (Rotrics Dexarm).
    pub fn arm(&self, name: impl Into<String>) -> AsyncArm<'_> {
        AsyncArm { client: self, name: name.into() }
    }

    /// Handle for a webcam net (MJPEG streaming).
    pub fn webcam(&self, name: impl Into<String>) -> AsyncWebcam<'_> {
        AsyncWebcam { client: self, name: name.into() }
    }

    /// Handle for a router net (MikroTik RouterOS).
    pub fn router(&self, name: impl Into<String>) -> AsyncRouter<'_> {
        AsyncRouter { client: self, name: name.into() }
    }

    /// Handle for the box's BLE adapter (box-level, not a saved net).
    pub fn ble(&self) -> AsyncBle<'_> {
        AsyncBle { client: self }
    }

    /// Handle for the box's WiFi interface (box-level, not a saved net).
    pub fn wifi(&self) -> AsyncWifi<'_> {
        AsyncWifi { client: self }
    }

    /// Handle for BluFi (ESP32 WiFi provisioning over BLE; box-level).
    pub fn blufi(&self) -> AsyncBlufi<'_> {
        AsyncBlufi { client: self }
    }

    /// Handle for box-side DFU via dfu-util (box-level, not a saved net).
    pub fn dfu(&self) -> AsyncDfu<'_> {
        AsyncDfu { client: self }
    }

    /// Handle for a debug-probe net (flash/erase/reset/read_memory).
    /// Talks to the box debug service on port 8765.
    pub fn debug(&self, name: impl Into<String>) -> AsyncDebugNet<'_> {
        AsyncDebugNet { client: self, name: name.into(), record: Default::default() }
    }

    /// Handle for an oscilloscope net. **Stub:** see [`Scope`].
    pub fn scope(&self, name: impl Into<String>) -> Scope {
        Scope::new(name)
    }
}
