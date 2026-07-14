//! Async client for the Lager box HTTP API (feature `async`).
//!
//! Runs the exact same request builders and response parsers as the
//! blocking client — only the transport differs.

use std::time::Duration;

use serde_json::Value;

use crate::error::{Error, Result};
use crate::nets::adc::AsyncAdc;
use crate::nets::arm::AsyncArm;
use crate::nets::battery::AsyncBattery;
use crate::nets::ble::AsyncBle;
use crate::nets::blufi::AsyncBlufi;
use crate::nets::dac::AsyncDac;
use crate::nets::debug::AsyncDebugNet;
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
use crate::wire::{self, BoxStatus, Health, HttpRequest, Method, NetRecord, Op, Timeout};

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
}

/// Builder for [`AsyncLagerBox`], for overriding the default timeout and the
/// debug-service URL.
pub struct AsyncLagerBoxBuilder {
    host: String,
    debug_url: Option<String>,
    default_timeout: Duration,
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

    /// Build the client.
    pub fn build(self) -> Result<AsyncLagerBox> {
        let base = wire::base_url(&self.host)?;
        let debug_base = match self.debug_url {
            Some(url) => wire::base_url_with_port(&url, wire::DEBUG_SERVICE_PORT)?,
            None => wire::service_base(&base, wire::DEBUG_SERVICE_PORT),
        };
        Ok(AsyncLagerBox {
            base,
            debug_base,
            http: reqwest::Client::new(),
            default_timeout: self.default_timeout,
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

    /// Send one request against an arbitrary base URL.
    async fn execute_at(&self, base: &str, req: &HttpRequest) -> Result<(u16, Value)> {
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
        let body: Value = resp.json().await.map_err(|e| {
            if e.is_timeout() {
                Error::Timeout(e.to_string())
            } else {
                Error::Decode(format!("non-JSON response: {e}"))
            }
        })?;
        Ok((status, body))
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

    /// Handle for a debug-probe net (flash/erase/reset/read_memory).
    /// Talks to the box debug service on port 8765.
    pub fn debug(&self, name: impl Into<String>) -> AsyncDebugNet<'_> {
        AsyncDebugNet { client: self, name: name.into() }
    }

    /// Handle for an oscilloscope net. **Stub:** see [`Scope`].
    pub fn scope(&self, name: impl Into<String>) -> Scope {
        Scope::new(name)
    }
}
