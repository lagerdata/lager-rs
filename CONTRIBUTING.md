# Contributing to lager-net

Thanks for your interest in contributing. This document covers how to get set up,
what CI checks before a pull request can merge, and how the repository is laid out.

## Getting Started

**Clone the repository and run the test suite.** Run the commands from the
repository root:

```bash
git clone https://github.com/lagerdata/lager-rs.git
cd lager-rs
cargo test
```

The crate's minimum supported Rust version is 1.75 (`rust-version` in
`Cargo.toml`). Develop on current stable. `cargo test` needs no box: the suite runs
against a mock box.

## What CI Checks

Every pull request runs the **CI** workflow, and a pull request cannot merge until
every check passes. Run the local equivalents before you push, to save a round trip.

| Check | Local command |
|-------|---------------|
| **fmt** | `cargo fmt --all --check` |
| **clippy** (3 feature sets) | `cargo clippy --locked --all-targets <features> -- -D warnings` |
| **test** (3 feature sets) | `cargo test --locked <features>` |
| **docs** | `RUSTDOCFLAGS="-D warnings" cargo doc --locked --no-deps --all-features` |
| **msrv** | Builds with the `rust-version` toolchain against an MSRV-aware resolve |

The three feature sets are:

```bash
cargo test --locked                                       # default (blocking)
cargo test --locked --no-default-features --features async
cargo test --locked --all-features                        # what docs.rs builds
```

**Run all three.** The features do not only add code: `async` alone leaves out the
blocking client, so a doctest or example that uses `LagerBox` breaks that build.
Gate such code with `#[cfg(feature = "blocking")]`, or with `required-features` for
an example.

To reproduce the **msrv** job:

```bash
CARGO_RESOLVER_INCOMPATIBLE_RUST_VERSIONS=fallback cargo +stable generate-lockfile
cargo +1.75 check --all-features --all-targets
```

This rewrites `Cargo.lock`. Do not commit the result: restore the committed lockfile
afterwards.

The **Supply chain** workflow runs `cargo deny` against the policy in `deny.toml`
when a pull request changes the dependencies. It is advisory, not a required check.
Run it locally with `cargo deny check`.

### Hardware tests (require a connected box)

`tests/hardware.rs` is `#[ignore]`d, so `cargo test` and CI never need a box. To run
it against a real box:

```bash
LAGER_BOX_HOST=<box-ip> cargo test --test hardware -- --ignored
# opt into net-specific tests:
LAGER_BOX_HOST=<box-ip> LAGER_TEST_SUPPLY_NET=supply1 cargo test --test hardware -- --ignored
```

A change that adds or changes a call to the box needs a hardware run. Say in the
pull request which box software version you ran it against.

## Pull Requests

1. **Fork** and branch from `main`. Use a short prefixed branch name, e.g.
   `fix/uart-read-timeout` or `feat/scope-capture`.
2. **Keep the change focused.** Unrelated fixes belong in their own PR.
3. **Add or update tests.** Box behavior is mocked with `httpmock`; see
   `tests/mock_api.rs` for the pattern.
4. **Update `CHANGELOG.md`** for any user-facing change. File the entry under
   `## [Unreleased]`, in the `###` section that fits (Added, Changed, Deprecated,
   Removed, Fixed, Security). If the change needs a newer box, give the minimum box
   version. Mark a change that can break existing callers as a **behavior change**,
   and say what callers must do.
5. **Do not bump the crate version.** The version in `Cargo.toml` changes only in a
   release pull request. See [RELEASE_PROCESS.md](RELEASE_PROCESS.md).
6. **Update the docs** if you changed the public API: rustdoc on the item, and
   `README.md` if it lists the feature or its minimum box version.
7. **Fill in the PR template**, including how you tested.

Note that this is a **public repository**. Pull request titles, bodies, commit
messages, and code comments are world-readable and permanent. The repository merges
with **Squash and merge**, so the pull request title and body become the commit
message on `main`. An edit to a PR body leaves the original text visible in its
revision history. Do not include customer names, private deployment details, or
internal discussion in anything you push.

### Code Style

- **Rust** -- `cargo fmt`, and zero `clippy` warnings on all three feature sets.
  Match the conventions of the file you edit.
- **Two clients** -- `LagerBox` (blocking) and `AsyncLagerBox` (async) expose the
  same API. A new method goes on both, and both share the request and response
  handling in `src/wire.rs`.
- **Errors** -- return `lager::Result<T>`. A box that predates an endpoint fails with
  `Error::UnsupportedByBox`, naming the box version required.
- **Commits** -- write a clear imperative subject line explaining the change.
- **No emoji** in code, docs, commit messages, or PR text. Use `PASS`/`FAIL`,
  `[x]`/`[ ]`, "Supported"/"Not supported".

## Reporting Issues

Use [GitHub Issues](https://github.com/lagerdata/lager-rs/issues) and the bug report
template. Search first -- the issue can already exist. A good report includes the
code that fails, the full error, the `lager-net` version and enabled features, your
`rustc --version`, and the box version from `lager hello --box <name>`.

A problem with the box itself, or with the `lager` CLI, belongs in
[lagerdata/lager](https://github.com/lagerdata/lager/issues).

For security vulnerabilities, **do not open a public issue.** Follow
[SECURITY.md](SECURITY.md).

## Repository Layout

```
lager-rs/
├── src/
│   ├── lib.rs            # crate docs and re-exports
│   ├── client.rs         # LagerBox, the blocking client (ureq)
│   ├── async_client.rs   # AsyncLagerBox, the async client (reqwest/tokio)
│   ├── wire.rs           # request bodies and response parsing, shared by both clients
│   ├── auth.rs           # gateway auth: token store, refresh, pinned tokens
│   ├── tunnel.rs         # debug_tunnel: HTTP CONNECT through a gateway
│   ├── error.rs          # the Error enum
│   └── nets/             # one module per net type (supply, gpio, uart, debug, ...)
├── tests/                # mock-box integration tests; hardware.rs needs a real box
├── examples/             # runnable examples (power_cycle, debug_tunnel)
└── MISSING_ENDPOINTS.md  # net types the box does not serve over HTTP yet
```

The crate publishes to crates.io as `lager-net`, but the library target is named
`lager`, so user code reads `use lager::LagerBox;`.

## Code of Conduct

See [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md). Report concerns to
hello@lagerdata.com.

## License

By contributing, you agree that your contributions are licensed under the
Apache License 2.0.
