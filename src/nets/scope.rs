//! Oscilloscope / logic-analyzer nets (Rigol MSO5000 family).
//!
//! **Not yet available over the box `:9000` HTTP API.** Scope capture and
//! streaming are large stateful workflows that still run over the legacy
//! `:5000` Python-exec path and the dedicated oscilloscope daemon. Every
//! method on this handle returns [`crate::Error::NotSupportedByBox`] until
//! the box exposes scope actions on `:9000` — see `MISSING_ENDPOINTS.md`
//! in this crate's repository for the endpoint sketch.

use crate::error::{Error, Result};

const FEATURE: &str = "scope";
const DETAILS: &str = "the box needs `POST :9000/net/command` scope/logic roles (or a \
     dedicated capture endpoint) for trigger config, single capture, and \
     measurement queries; see MISSING_ENDPOINTS.md";

fn unsupported<T>() -> Result<T> {
    Err(Error::NotSupportedByBox {
        feature: FEATURE,
        details: DETAILS,
    })
}

/// Handle for an oscilloscope (analog or logic) net. **Stub:** every method
/// currently returns [`Error::NotSupportedByBox`].
#[derive(Debug, Clone)]
pub struct Scope {
    name: String,
}

impl Scope {
    pub(crate) fn new(name: impl Into<String>) -> Self {
        Scope { name: name.into() }
    }

    /// Name of the net this handle drives.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Enable the channel this net maps to.
    pub fn enable(&self) -> Result<()> {
        unsupported()
    }

    /// Disable the channel this net maps to.
    pub fn disable(&self) -> Result<()> {
        unsupported()
    }

    /// Perform a single triggered capture and return the trace.
    pub fn capture(&self) -> Result<Vec<f64>> {
        unsupported()
    }

    /// Query a scalar measurement (e.g. `"vpp"`, `"frequency"`).
    pub fn measure(&self, _item: &str) -> Result<f64> {
        unsupported()
    }
}
