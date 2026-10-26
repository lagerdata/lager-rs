//! YubiHSM 2 nets (box >= 0.59.0): key generation, signing and object
//! management on a hardware security module attached to the box.
//!
//! Every action except [`Hsm::info`] logs in to the HSM, with an
//! [`HsmAuth`]. The box never logs or echoes the password, and `HsmAuth`'s
//! `Debug` output leaves it out.
//!
//! ```no_run
//! # #[cfg(feature = "blocking")]
//! # fn demo() -> lager::Result<()> {
//! use lager::{GenerateKey, HsmAuth, LagerBox};
//!
//! let lager = LagerBox::from_env()?;
//! let hsm = lager.hsm("hsm1");
//! let auth = HsmAuth::new(std::env::var("HSM_PASSWORD").unwrap());
//! let key = hsm.generate(&auth, &GenerateKey::new("ec-p256"))?;
//! let signature = hsm.sign(&auth, key.id, b"firmware digest", "ecdsa-sha256")?;
//! println!("{}\n{} signature bytes", key.public_key_pem, signature.len());
//! # Ok(())
//! # }
//! # fn main() {}
//! ```
//!
//! An unknown algorithm, mechanism or object type fails with
//! [`crate::Error::Box`] (HTTP 400) listing the valid names. A failed login
//! or an HSM error is HTTP 502.

use std::fmt;

use serde::Deserialize;

use super::net_handle;

/// Login credentials for an HSM action: an authentication key ID and its
/// password. `Debug` output omits the password.
#[derive(Clone)]
pub struct HsmAuth {
    auth_key_id: u16,
    password: String,
}

impl HsmAuth {
    /// Log in with authentication key 1, the factory default key.
    pub fn new(password: impl Into<String>) -> Self {
        HsmAuth::with_key(1, password)
    }

    /// Log in with the given authentication key.
    pub fn with_key(auth_key_id: u16, password: impl Into<String>) -> Self {
        HsmAuth {
            auth_key_id,
            password: password.into(),
        }
    }

    /// The authentication key this logs in with.
    pub fn auth_key_id(&self) -> u16 {
        self.auth_key_id
    }
}

impl fmt::Debug for HsmAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HsmAuth")
            .field("auth_key_id", &self.auth_key_id)
            .field("password", &"<redacted>")
            .finish()
    }
}

/// The device's identity, from [`Hsm::info`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[non_exhaustive]
pub struct HsmInfo {
    /// Firmware version (`"2.4.0"`).
    pub firmware: String,
    /// Serial number.
    pub serial: u64,
    /// Audit-log entries in use.
    pub log_used: u32,
    /// Audit-log capacity.
    pub log_size: u32,
    /// Part number, when the device reports one.
    #[serde(default)]
    pub part_number: Option<String>,
    /// Algorithms the device supports (`"ec-p256"`, `"ed25519"`, ...).
    #[serde(default)]
    pub algorithms: Vec<String>,
}

/// One object stored on the HSM, from [`Hsm::objects`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[non_exhaustive]
pub struct HsmObject {
    /// Object ID.
    pub id: u16,
    /// Object type (`"asymmetric-key"`, `"authentication-key"`, ...).
    #[serde(rename = "type")]
    pub object_type: String,
    /// Label.
    #[serde(default)]
    pub label: String,
    /// Algorithm, for a key.
    #[serde(default)]
    pub algorithm: Option<String>,
    /// Domains the object belongs to (1-16).
    #[serde(default)]
    pub domains: Vec<u8>,
    /// Capabilities (`"sign-ecdsa"`, `"exportable-under-wrap"`, ...).
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// Size in bytes.
    #[serde(default)]
    pub size: u32,
    /// Sequence number: how many times this ID has been reused.
    #[serde(default)]
    pub sequence: u32,
}

/// What [`Hsm::generate`] creates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenerateKey {
    /// Algorithm: `"ec-p256"`, `"ec-p384"`, `"ec-p521"`, `"ec-k256"` or
    /// `"ed25519"`. The box refuses RSA, whose generation can take minutes.
    pub algorithm: String,
    /// Object ID to store the key under; 0 lets the HSM pick one.
    pub object_id: u16,
    /// Label.
    pub label: String,
    /// Domains (1-16); empty means domain 1.
    pub domains: Vec<u8>,
    /// Capabilities; empty means the box's defaults for the algorithm.
    pub capabilities: Vec<String>,
}

impl GenerateKey {
    /// A key of this algorithm, with every other setting at its default.
    pub fn new(algorithm: impl Into<String>) -> Self {
        GenerateKey {
            algorithm: algorithm.into(),
            object_id: 0,
            label: String::new(),
            domains: Vec::new(),
            capabilities: Vec::new(),
        }
    }
}

/// A key [`Hsm::generate`] created.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[non_exhaustive]
pub struct GeneratedKey {
    /// The key's object ID.
    pub id: u16,
    /// Its public key, as PEM (SubjectPublicKeyInfo).
    pub public_key_pem: String,
}

pub(crate) mod ops {
    use std::time::Duration;

    use serde::Deserialize;
    use serde_json::{json, Value};

    use super::{GenerateKey, GeneratedKey, HsmAuth, HsmInfo, HsmObject};
    use crate::error::{Error, Result};
    use crate::nets::debug::{base64_decode, base64_encode};
    use crate::wire::{net_command, unit, value_as, CommandResponse, Op, Timeout};

    const ROLE: &str = "hsm";

    /// Each action opens the device, logs in and runs one operation.
    fn budget() -> Timeout {
        Timeout::After(Duration::from_secs(30))
    }

    /// `params` plus the login fields.
    fn with_auth(auth: &HsmAuth, mut params: Value) -> Value {
        params["auth_key_id"] = json!(auth.auth_key_id);
        params["password"] = json!(auth.password);
        params
    }

    fn cmd<T>(
        name: &str,
        action: &str,
        params: Value,
        timeout: Timeout,
        parse: fn(CommandResponse) -> Result<T>,
    ) -> Op<T> {
        Op {
            req: net_command(name, ROLE, action, params, timeout),
            parse,
        }
    }

    fn parse_string(resp: CommandResponse) -> Result<String> {
        match resp.value {
            Some(Value::String(s)) => Ok(s),
            other => Err(Error::Decode(format!(
                "expected string 'value', got {other:?}"
            ))),
        }
    }

    #[derive(Deserialize)]
    struct Signature {
        signature: String,
    }

    fn parse_signature(resp: CommandResponse) -> Result<Vec<u8>> {
        let sig: Signature = value_as(resp)?;
        base64_decode(&sig.signature)
            .ok_or_else(|| Error::Decode("'signature' is not valid base64".to_string()))
    }

    pub(crate) fn info(name: &str) -> Op<HsmInfo> {
        cmd(name, "info", json!({}), budget(), value_as::<HsmInfo>)
    }

    pub(crate) fn blink(name: &str, auth: &HsmAuth, seconds: u8) -> Op<()> {
        cmd(
            name,
            "blink",
            with_auth(auth, json!({ "seconds": seconds })),
            // The box waits out the blink before answering.
            Timeout::After(Duration::from_secs(u64::from(seconds) + 30)),
            unit,
        )
    }

    pub(crate) fn objects(
        name: &str,
        auth: &HsmAuth,
        object_type: Option<&str>,
        label: Option<&str>,
    ) -> Op<Vec<HsmObject>> {
        let mut params = json!({});
        if let Some(t) = object_type {
            params["type"] = json!(t);
        }
        if let Some(l) = label {
            params["label"] = json!(l);
        }
        cmd(
            name,
            "objects",
            with_auth(auth, params),
            budget(),
            value_as::<Vec<HsmObject>>,
        )
    }

    pub(crate) fn delete(
        name: &str,
        auth: &HsmAuth,
        object_id: u16,
        object_type: Option<&str>,
    ) -> Op<()> {
        let mut params = json!({ "object_id": object_id });
        if let Some(t) = object_type {
            params["object_type"] = json!(t);
        }
        cmd(name, "delete", with_auth(auth, params), budget(), unit)
    }

    pub(crate) fn generate(name: &str, auth: &HsmAuth, key: &GenerateKey) -> Op<GeneratedKey> {
        let mut params = json!({
            "algorithm": key.algorithm,
            "object_id": key.object_id,
            "label": key.label,
        });
        if !key.domains.is_empty() {
            params["domains"] = json!(key.domains);
        }
        if !key.capabilities.is_empty() {
            params["capabilities"] = json!(key.capabilities);
        }
        cmd(
            name,
            "generate",
            with_auth(auth, params),
            budget(),
            value_as::<GeneratedKey>,
        )
    }

    pub(crate) fn public_key(name: &str, auth: &HsmAuth, object_id: u16) -> Op<String> {
        cmd(
            name,
            "pubkey",
            with_auth(auth, json!({ "object_id": object_id })),
            budget(),
            parse_string,
        )
    }

    pub(crate) fn sign(
        name: &str,
        auth: &HsmAuth,
        object_id: u16,
        data: &[u8],
        mechanism: &str,
    ) -> Op<Vec<u8>> {
        let params = json!({
            "object_id": object_id,
            "data": base64_encode(data),
            "mechanism": mechanism,
        });
        cmd(
            name,
            "sign",
            with_auth(auth, params),
            budget(),
            parse_signature,
        )
    }

    pub(crate) fn reset(name: &str, auth: &HsmAuth) -> Op<()> {
        cmd(
            name,
            "reset",
            with_auth(auth, json!({})),
            // The box allows the reboot 60 s.
            Timeout::After(Duration::from_secs(75)),
            unit,
        )
    }
}

net_handle! {
    /// Handle for a YubiHSM 2 net. See the [module docs](self).
    sync: Hsm,
    async: AsyncHsm,
    methods: {
        /// Firmware, serial number, audit-log use and supported algorithms.
        /// Needs no login.
        fn info() -> HsmInfo = ops::info;
        /// Blink the device's LED for `seconds`, to find it on the bench.
        fn blink(auth: &HsmAuth, seconds: u8) -> () = ops::blink;
        /// Stored objects, optionally only one type (`"asymmetric-key"`, ...)
        /// or label.
        fn objects(auth: &HsmAuth, object_type: Option<&str>, label: Option<&str>) -> Vec<HsmObject> = ops::objects;
        /// Delete an object. `object_type` defaults to `"asymmetric-key"`.
        fn delete(auth: &HsmAuth, object_id: u16, object_type: Option<&str>) -> () = ops::delete;
        /// Generate an asymmetric key on the device and return its public
        /// key.
        fn generate(auth: &HsmAuth, key: &GenerateKey) -> GeneratedKey = ops::generate;
        /// A stored key's public key, as PEM.
        fn public_key(auth: &HsmAuth, object_id: u16) -> String = ops::public_key;
        /// Sign `data` with a stored key and return the signature.
        /// `mechanism` is one of `"ecdsa-sha256"`, `"ecdsa-sha384"`,
        /// `"ecdsa-sha512"`, `"eddsa"`, `"rsa-pkcs1-sha256"` (and `-sha384`,
        /// `-sha512`) or `"rsa-pss-sha256"` (and `-sha384`, `-sha512`).
        fn sign(auth: &HsmAuth, object_id: u16, data: &[u8], mechanism: &str) -> Vec<u8> = ops::sign;
        /// **Factory-reset the device.** Deletes every key and object and
        /// restores the factory login (key 1, password `password`). Cannot
        /// be undone.
        fn reset(auth: &HsmAuth) -> () = ops::reset;
    }
}
