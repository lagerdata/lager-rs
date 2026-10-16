# Security Policy

## Supported Versions

`lager-net` is pre-1.0 and ships new minor versions often. Rather than pin a table
that goes stale each release, the policy is:

| Release | Support |
| --- | --- |
| Latest minor (see [crates.io](https://crates.io/crates/lager-net)) | Full support - bug fixes and security fixes |
| Previous minor | Security fixes only |
| Older | Not supported - please upgrade |

Upgrade with `cargo update -p lager-net`, or raise the `version` in your
`Cargo.toml` for a new minor.

## Reporting a Vulnerability

We take security seriously. If you discover a vulnerability in `lager-net`, please
report it responsibly.

### How to Report

1. **Do not** open a public GitHub issue for security vulnerabilities.
2. Report the vulnerability privately through GitHub. On the repository's
   **Security** tab, select **Report a vulnerability**. You can also open the
   [private reporting form](https://github.com/lagerdata/lager-rs/security/advisories/new)
   directly.
3. If you cannot use GitHub, email **hello@lagerdata.com**.
4. Include as much detail as you can:
   - Description of the vulnerability
   - Steps to reproduce
   - Potential impact
   - Affected version (the `lager-net` version in your `Cargo.lock`) and enabled
     features
   - Box software version, if a box is involved
   - Any suggested fix (optional)

### What to Expect

- **Acknowledgment** -- within 48 hours.
- **Assessment** -- we confirm the issue and determine severity.
- **Updates** -- we keep you informed of progress.
- **Resolution** -- we aim to resolve critical issues within 30 days.
- **Credit** -- with your permission, we credit you in the security advisory
  and the release notes.

### Scope

This policy applies to the `lager-net` crate in this repository, in particular:

- Gateway credentials: how the crate reads, refreshes, and writes the CLI's token
  store (`~/.lager_gateway_auth`, or `LAGER_GATEWAY_AUTH_FILE`), and how it handles
  a pinned token (`LagerBoxBuilder::bearer_token`, `LAGER_GATEWAY_TOKEN`).
- Where a token is sent: which hosts and ports the crate attaches
  `Authorization: Bearer` to, and the `CONNECT` tunnel that `debug_tunnel` opens.
- Parsing of box and gateway responses.

### Out of Scope

- The box software, its HTTP services and the `lager` CLI. Report those to
  [lagerdata/lager](https://github.com/lagerdata/lager/security), whose
  [SECURITY.md](https://github.com/lagerdata/lager/blob/main/SECURITY.md) also
  records the box threat model.
- Third-party dependencies -- report to their upstream maintainers.
- Issues in user test code or box configurations.
- Physical security of bench hardware.

## Security Model

The crate is a client. It has no more authority than the box it talks to, and a box
is trusted infrastructure on a trusted network: see the
[lager security model](https://github.com/lagerdata/lager/blob/main/SECURITY.md#security-model).

- **A plain box is unauthenticated.** Anyone who can reach its ports can drive its
  instruments. The crate sends no credential to it.
- **A box behind an authenticating gateway** gets a bearer token on every request.
  The token is short-lived, and the crate refreshes it from the CLI's session.
  Treat the token store like an SSH key: the crate writes it with mode `0600`, as
  the CLI does.
- **Prefer a pinned token in CI** over copying a developer's token store to a
  runner. Pass it from a secret with `LAGER_GATEWAY_TOKEN`.
- **Box traffic is plain HTTP by default.** A bearer token does not encrypt the connection.
  Reach boxes over a VPN or a trusted LAN.
