---
name: Bug Report
about: Report a bug in the lager-net crate
title: ''
labels: bug
assignees: ''
---

**Describe the bug**
A clear and concise description of what the bug is.

**To reproduce**
The smallest code that shows the problem:

```rust
let lager = lager::LagerBox::from_env()?;
// ...
```

**Expected behavior**
A clear and concise description of what you expected to happen.

**Actual result**
Paste the full error (its `Display` and `Debug` output), panic message, or test output.

**Environment**
- OS: [e.g., macOS 14.5, Ubuntu 22.04]
- Rust version: [output of `rustc --version`]
- lager-net version: [from your `Cargo.lock`]
- Features: [e.g., default, `async`, `uart`, `rtt`]
- Client: [blocking `LagerBox` or async `AsyncLagerBox`]
- Box software version: [output of `lager hello --box <name>`]
- Box behind an authenticating gateway: [yes / no]

**Additional context**
Add any other context about the problem here (instrument model, net configuration, etc.).
A problem with the box itself or the `lager` CLI belongs in
[lagerdata/lager](https://github.com/lagerdata/lager/issues).
