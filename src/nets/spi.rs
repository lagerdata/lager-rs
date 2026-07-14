//! SPI bus nets.
//!
//! Bus transactions run box-side under the physical device's shared lock,
//! so a full transfer can never interleave with a concurrent request on the
//! same device (e.g. parallel cargo tests).

use serde::Deserialize;
use serde_json::{Map, Value};

use super::net_handle;
use crate::wire::lenient;

/// SPI bit order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BitOrder {
    /// Most-significant bit first.
    Msb,
    /// Least-significant bit first.
    Lsb,
}

impl BitOrder {
    /// Wire encoding.
    pub fn as_str(&self) -> &'static str {
        match self {
            BitOrder::Msb => "msb",
            BitOrder::Lsb => "lsb",
        }
    }
}

/// Chip-select active polarity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CsActive {
    /// CS is active-low (typical).
    Low,
    /// CS is active-high.
    High,
}

impl CsActive {
    /// Wire encoding.
    pub fn as_str(&self) -> &'static str {
        match self {
            CsActive::Low => "low",
            CsActive::High => "high",
        }
    }
}

/// Chip-select handling mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CsMode {
    /// The driver asserts/deasserts CS around each transaction.
    Auto,
    /// The caller controls CS (e.g. via a GPIO net).
    Manual,
}

impl CsMode {
    /// Wire encoding.
    pub fn as_str(&self) -> &'static str {
        match self {
            CsMode::Auto => "auto",
            CsMode::Manual => "manual",
        }
    }
}

/// Configuration overrides for [`Spi::configure`]. `None` fields keep the
/// net's saved value; explicit fields are applied to the live driver and
/// persisted on the box.
#[derive(Debug, Clone, Copy, Default)]
pub struct SpiConfig {
    /// SPI mode 0–3 (CPOL/CPHA).
    pub mode: Option<u8>,
    /// Bit order.
    pub bit_order: Option<BitOrder>,
    /// Clock frequency in Hz.
    pub frequency_hz: Option<u32>,
    /// Word size in bits (8, 16, or 32).
    pub word_size: Option<u8>,
    /// CS active polarity.
    pub cs_active: Option<CsActive>,
    /// CS handling mode.
    pub cs_mode: Option<CsMode>,
}

/// The effective SPI configuration the box reports after `configure`.
#[derive(Debug, Clone, Deserialize)]
pub struct SpiEffectiveConfig {
    /// SPI mode 0–3.
    #[serde(default, deserialize_with = "lenient::opt_i64")]
    pub mode: Option<i64>,
    /// Clock frequency in Hz.
    #[serde(default, deserialize_with = "lenient::opt_i64")]
    pub frequency_hz: Option<i64>,
    /// Word size in bits.
    #[serde(default, deserialize_with = "lenient::opt_i64")]
    pub word_size: Option<i64>,
    /// Bit order (`"msb"`/`"lsb"`).
    #[serde(default)]
    pub bit_order: Option<String>,
    /// CS active polarity (`"low"`/`"high"`).
    #[serde(default)]
    pub cs_active: Option<String>,
    /// CS handling mode (`"auto"`/`"manual"`).
    #[serde(default)]
    pub cs_mode: Option<String>,
    /// Any extra driver-specific fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Result of an SPI transaction: the words clocked in, plus the word size
/// they were clocked at.
#[derive(Debug, Clone)]
pub struct SpiTransfer {
    /// Words read back (one entry per word, regardless of word size).
    pub words: Vec<u32>,
    /// Word size in bits the transaction ran at.
    pub word_size: u32,
}

/// Per-transaction options. The defaults match the CLI: fill `0xFF`,
/// CS released at the end of the transaction.
#[derive(Debug, Clone, Copy)]
pub struct SpiOptions {
    /// Fill word clocked out when reading (or padding a transfer).
    pub fill: u32,
    /// Keep CS asserted after the transaction (for multi-part transfers).
    pub keep_cs: bool,
}

impl Default for SpiOptions {
    fn default() -> Self {
        SpiOptions {
            fill: 0xFF,
            keep_cs: false,
        }
    }
}

pub(crate) mod ops {
    use serde_json::{json, Map, Value};

    use super::{SpiConfig, SpiEffectiveConfig, SpiOptions, SpiTransfer};
    use crate::error::{Error, Result};
    use crate::wire::{
        as_i64, net_command, value_as, value_int_list, CommandResponse, Op, Timeout,
    };

    const ROLE: &str = "spi";

    fn parse_transfer(resp: CommandResponse) -> Result<SpiTransfer> {
        let word_size = resp
            .extra
            .get("word_size")
            .and_then(as_i64)
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| Error::Decode("SPI response missing 'word_size'".to_string()))?;
        let words = value_int_list(resp)?;
        Ok(SpiTransfer { words, word_size })
    }

    pub(crate) fn configure(name: &str, config: &SpiConfig) -> Op<SpiEffectiveConfig> {
        let mut params = Map::new();
        if let Some(m) = config.mode {
            params.insert("mode".to_string(), json!(m));
        }
        if let Some(b) = config.bit_order {
            params.insert("bit_order".to_string(), json!(b.as_str()));
        }
        if let Some(f) = config.frequency_hz {
            params.insert("frequency_hz".to_string(), json!(f));
        }
        if let Some(w) = config.word_size {
            params.insert("word_size".to_string(), json!(w));
        }
        if let Some(c) = config.cs_active {
            params.insert("cs_active".to_string(), json!(c.as_str()));
        }
        if let Some(c) = config.cs_mode {
            params.insert("cs_mode".to_string(), json!(c.as_str()));
        }
        Op {
            req: net_command(name, ROLE, "config", Value::Object(params), Timeout::Default),
            parse: value_as::<SpiEffectiveConfig>,
        }
    }

    pub(crate) fn read(name: &str, n_words: u32, opts: &SpiOptions) -> Op<SpiTransfer> {
        Op {
            req: net_command(
                name,
                ROLE,
                "read",
                json!({ "n_words": n_words, "fill": opts.fill, "keep_cs": opts.keep_cs }),
                Timeout::Default,
            ),
            parse: parse_transfer,
        }
    }

    pub(crate) fn write(name: &str, data: &[u32], opts: &SpiOptions) -> Op<SpiTransfer> {
        // "write" is a full-duplex transfer box-side; the words clocked in
        // are returned.
        Op {
            req: net_command(
                name,
                ROLE,
                "write",
                json!({ "data": data, "keep_cs": opts.keep_cs }),
                Timeout::Default,
            ),
            parse: parse_transfer,
        }
    }

    pub(crate) fn read_write(name: &str, data: &[u32], opts: &SpiOptions) -> Op<SpiTransfer> {
        Op {
            req: net_command(
                name,
                ROLE,
                "read_write",
                json!({ "data": data, "keep_cs": opts.keep_cs }),
                Timeout::Default,
            ),
            parse: parse_transfer,
        }
    }

    pub(crate) fn transfer(
        name: &str,
        data: &[u32],
        n_words: u32,
        opts: &SpiOptions,
    ) -> Op<SpiTransfer> {
        Op {
            req: net_command(
                name,
                ROLE,
                "transfer",
                json!({
                    "data": data,
                    "n_words": n_words,
                    "fill": opts.fill,
                    "keep_cs": opts.keep_cs,
                }),
                Timeout::Default,
            ),
            parse: parse_transfer,
        }
    }
}

net_handle! {
    /// Handle for an SPI bus net.
    sync: Spi,
    async: AsyncSpi,
    methods: {
        /// Apply configuration overrides (persisted on the box) and return
        /// the effective configuration.
        fn configure(config: &SpiConfig) -> SpiEffectiveConfig = ops::configure;
        /// Clock in `n_words` words, clocking out the fill word.
        fn read(n_words: u32, opts: &SpiOptions) -> SpiTransfer = ops::read;
        /// Clock out exactly `data` (full duplex; the words clocked in are
        /// returned).
        fn write(data: &[u32], opts: &SpiOptions) -> SpiTransfer = ops::write;
        /// Full-duplex transfer of exactly `data`.
        fn read_write(data: &[u32], opts: &SpiOptions) -> SpiTransfer = ops::read_write;
        /// Transfer `n_words` words, padding/truncating `data` with the
        /// fill word.
        fn transfer(data: &[u32], n_words: u32, opts: &SpiOptions) -> SpiTransfer = ops::transfer;
    }
}
