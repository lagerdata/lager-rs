//! Helpers shared by the Socket.IO streaming sessions ([`super::uart`] and
//! [`super::rtt`]). Both namespaces speak the same dialect — data rides as
//! lowercase hex strings under a `data` key — so the codec lives here once
//! and the two sessions cannot drift.

use rust_socketio::Payload;
use serde_json::Value;

pub(crate) fn hex_encode(data: &[u8]) -> String {
    let mut s = String::with_capacity(data.len() * 2);
    for b in data {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

pub(crate) fn hex_decode(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

/// Pull the first JSON object out of a Socket.IO payload.
pub(crate) fn payload_json(payload: Payload) -> Option<Value> {
    match payload {
        Payload::Text(values) => values.into_iter().next(),
        #[allow(deprecated)]
        Payload::String(s) => serde_json::from_str(&s).ok(),
        Payload::Binary(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{hex_decode, hex_encode};

    #[test]
    fn hex_roundtrip() {
        let data = [0x00, 0x0a, 0xff, 0x42];
        assert_eq!(hex_encode(&data), "000aff42");
        assert_eq!(hex_decode(&hex_encode(&data)).unwrap(), data);
        assert!(hex_decode("zz").is_none());
        assert!(hex_decode("abc").is_none());
    }
}
