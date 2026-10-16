## Summary

Brief description of the changes in this PR.

## Changes

-

## Testing

Describe how you tested these changes:
- [ ] `cargo test` passes for all three feature sets (default, `--no-default-features --features async`, `--all-features`)
- [ ] Ran `tests/hardware.rs` against a box (give the box software version and which tests)
- [ ] Changes are documentation-only (no testing needed)

## Checklist

- [ ] `cargo fmt` and `cargo clippy -- -D warnings` are clean
- [ ] Tests added or updated
- [ ] New methods are on both `LagerBox` and `AsyncLagerBox`
- [ ] CHANGELOG.md updated under `## [Unreleased]` (if user-facing change)
- [ ] Minimum box version stated in the rustdoc and `README.md` (if the change needs a newer box)
- [ ] No version bump (releases bump the version, see RELEASE_PROCESS.md)
