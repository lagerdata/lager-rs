//! Debug tunnels: raw TCP to a box's debug ports through its gateway.
//!
//! The debug servers on a box (GDB, OpenOCD telnet/TCL, RTT) speak raw TCP,
//! which an authenticating gateway cannot check a bearer token on, so a
//! gated box does not publish those ports. The gateway instead accepts an
//! HTTP `CONNECT` on the debug-service port (8765) and, once the request is
//! authorized, splices the connection to that port in the Lager container.
//! The handshake is §10 of `docs/reference/gateway-auth-contract.md` in the
//! Lager monorepo; the CLI's implementation is `cli/gateway_tunnel.py`.
//!
//! This module is the transport-independent half: building the request,
//! parsing the response head, and classifying a refusal. The blocking and
//! async clients each drive the socket themselves.

use crate::error::{Error, Result};

/// A response head larger than this is not a gateway talking to us.
pub(crate) const MAX_HEAD_BYTES: usize = 64 * 1024;

/// How much of a refusal's body is read for the error message.
pub(crate) const MAX_BODY_BYTES: usize = 4096;

/// The `CONNECT` request for `port`, with the box's own bearer token.
///
/// The request target is the bare port, which the contract allows: the
/// gateway ignores any host part and only ever reaches the Lager container.
pub(crate) fn connect_request(port: u16, box_host: &str, token: Option<&str>) -> Vec<u8> {
    let mut req = format!("CONNECT {port} HTTP/1.1\r\nHost: {box_host}:{port}\r\n");
    if let Some(token) = token {
        req.push_str(&format!("Authorization: Bearer {token}\r\n"));
    }
    req.push_str("\r\n");
    req.into_bytes()
}

/// The parts of a response head the client acts on.
#[derive(Debug)]
pub(crate) struct Head {
    pub(crate) status: u16,
    /// `X-Gateway-Auth-Url`, when present.
    pub(crate) discovery: Option<String>,
    pub(crate) content_length: usize,
}

/// Parse a complete response head (everything up to and including the
/// blank line). `None` when it is not an HTTP response at all.
pub(crate) fn parse_head(raw: &[u8]) -> Option<Head> {
    let text = std::str::from_utf8(raw).ok()?;
    let mut lines = text.split("\r\n");
    let mut status_line = lines.next()?.splitn(3, ' ');
    if !status_line.next()?.starts_with("HTTP/") {
        return None;
    }
    let status = status_line.next()?.parse().ok()?;
    let mut discovery = None;
    let mut content_length = 0;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        if name
            .trim()
            .eq_ignore_ascii_case(crate::auth::DISCOVERY_HEADER)
        {
            discovery = Some(value.to_string());
        } else if name.trim().eq_ignore_ascii_case("content-length") {
            content_length = value.parse().unwrap_or(0);
        }
    }
    Some(Head {
        status,
        discovery,
        content_length,
    })
}

/// What one `CONNECT` attempt came to, short of an open tunnel.
#[derive(Debug)]
pub(crate) enum Refusal {
    /// 401/403/503 carrying the discovery header: a gateway denial, handled
    /// like every other request's (contract §6.3).
    Denied { status: u16, auth_url: String },
    /// 403 without the discovery header: a port outside the debug ranges.
    NotDebugPort { body: String },
    /// 502: the gateway reached the container, but nothing listens there.
    NothingListening { body: String },
    /// Anything else. The service on 8765 does not do `CONNECT`: the box's
    /// own debug service (a plain box answers 501), or a gateway that
    /// predates tunnels.
    Unsupported { status: Option<u16> },
}

/// Classify a non-200 head (and the start of its body).
pub(crate) fn classify(head: &Head, body: String) -> Refusal {
    if crate::auth::is_denial(head.status) {
        if let Some(auth_url) = &head.discovery {
            return Refusal::Denied {
                status: head.status,
                auth_url: auth_url.clone(),
            };
        }
    }
    match head.status {
        403 => Refusal::NotDebugPort { body },
        502 => Refusal::NothingListening { body },
        status => Refusal::Unsupported {
            status: Some(status),
        },
    }
}

/// The error for a refusal that is not a gateway denial.
pub(crate) fn refusal_error(refusal: &Refusal, box_host: &str, port: u16) -> Error {
    let detail = |body: &str| {
        if body.is_empty() {
            String::new()
        } else {
            format!(" ({body})")
        }
    };
    match refusal {
        Refusal::NotDebugPort { body } => Error::Box {
            status: 403,
            message: format!(
                "the gateway of box {box_host} does not tunnel port {port}{}: gateways \
                 tunnel only the debug ports (GDB 2331-2342, OpenOCD 4444-4447 and \
                 6666-6669, RTT 9090-9097)",
                detail(body)
            ),
        },
        Refusal::NothingListening { body } => Error::Box {
            status: 502,
            message: format!(
                "nothing is listening on port {port} on box {box_host}{}: start the \
                 debug server first (DebugNet::connect)",
                detail(body)
            ),
        },
        Refusal::Unsupported { status } => Error::Box {
            status: status.unwrap_or(0),
            message: format!(
                "port {port} on box {box_host} is reachable only through its gateway, \
                 and that gateway does not support debug tunnels yet; ask the box's \
                 administrator to update it"
            ),
        },
        // Denials are turned into errors by `auth::denial_error`.
        Refusal::Denied { status, auth_url } => {
            crate::auth::denial_error(*status, box_host, auth_url, true)
        }
    }
}

/// Split a normalized base URL (`http://host:port`) into host and port.
pub(crate) fn host_port(base: &str) -> Result<(String, u16)> {
    let rest = base.split_once("://").map(|(_, r)| r).unwrap_or(base);
    let rest = rest.split('/').next().unwrap_or(rest);
    let (host, port) = rest
        .rsplit_once(':')
        .ok_or_else(|| Error::Config(format!("no port in debug-service URL '{base}'")))?;
    let port = port
        .parse()
        .map_err(|_| Error::Config(format!("bad port in debug-service URL '{base}'")))?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    Ok((host.to_string(), port))
}

/// Map a socket error from the handshake to the crate's error type.
pub(crate) fn io_error(context: &str, e: std::io::Error) -> Error {
    match e.kind() {
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => {
            Error::Timeout(format!("{context}: {e}"))
        }
        _ => Error::Connection(format!("{context}: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_uses_the_bare_port_and_the_token() {
        let req = String::from_utf8(connect_request(2331, "box1", Some("t"))).unwrap();
        assert_eq!(
            req,
            "CONNECT 2331 HTTP/1.1\r\nHost: box1:2331\r\nAuthorization: Bearer t\r\n\r\n"
        );
        let bare = String::from_utf8(connect_request(2331, "box1", None)).unwrap();
        assert!(!bare.contains("Authorization"));
    }

    #[test]
    fn head_parsing() {
        let head = parse_head(
            b"HTTP/1.1 401 Unauthorized\r\nX-Gateway-Auth-Url: https://auth\r\n\
              Content-Length: 12\r\n\r\n",
        )
        .unwrap();
        assert_eq!(head.status, 401);
        assert_eq!(head.discovery.as_deref(), Some("https://auth"));
        assert_eq!(head.content_length, 12);
        assert!(parse_head(b"SSH-2.0-OpenSSH\r\n\r\n").is_none());
    }

    #[test]
    fn classification() {
        let head = |status, discovery: Option<&str>| Head {
            status,
            discovery: discovery.map(str::to_string),
            content_length: 0,
        };
        assert!(matches!(
            classify(&head(403, Some("https://a")), String::new()),
            Refusal::Denied { status: 403, .. }
        ));
        assert!(matches!(
            classify(&head(403, None), String::new()),
            Refusal::NotDebugPort { .. }
        ));
        assert!(matches!(
            classify(&head(502, None), String::new()),
            Refusal::NothingListening { .. }
        ));
        assert!(matches!(
            classify(&head(501, None), String::new()),
            Refusal::Unsupported { status: Some(501) }
        ));
    }

    #[test]
    fn host_port_split() {
        assert_eq!(
            host_port("http://192.168.1.42:8765").unwrap(),
            ("192.168.1.42".to_string(), 8765)
        );
        assert_eq!(
            host_port("http://[::1]:8765").unwrap(),
            ("::1".to_string(), 8765)
        );
        assert!(host_port("http://box").is_err());
    }
}
