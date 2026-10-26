# Release Process for lager-net

This document is for **maintainers** who publish releases of the `lager-net` crate to
crates.io.

To contribute code, see [CONTRIBUTING.md](CONTRIBUTING.md).

## How a Release Works

A release is a version tag, `vX.Y.Z`, on a commit on `main`. Pushing the tag starts the
**Release** workflow (`.github/workflows/release.yml`):

1. **verify** checks that the tag matches the `version` in `Cargo.toml`, that the tagged
   commit is on `main`, and that `CHANGELOG.md` has a non-empty `## [X.Y.Z]` section.
   Then it runs `cargo publish --locked --dry-run`.
2. **publish** waits for a reviewer to approve the `crates-io` environment. It then
   publishes to crates.io and creates the GitHub Release, with the version's
   CHANGELOG section as the release notes.

The publish job uses crates.io trusted publishing. It exchanges the job's GitHub OIDC
token for a short-lived publish token, so no registry token is stored in the
repository.

**A crates.io version is permanent.** It can be yanked, but its number can never be
used again. Everything that can be checked is checked before the approval gate. Do not
approve the deployment until **verify** is green and you have read the release notes
it extracted.

## When to Release

The crate has its own version numbers. They do not follow the box software's version.
Release the crate when it has a user-facing change, and in particular when a box
release changes the box HTTP API.

Each box release tag runs **Release: lager-rs API check** in lagerdata/lager. When the
box HTTP API changed since the previous box release, that workflow opens an issue here,
titled `Box API changes in lager vX.Y.Z`. The issue lists the new and removed routes,
`/net/command` roles and actions, debug service features and `/status` capabilities.
For each item, decide whether the crate picks it up. `MISSING_ENDPOINTS.md` records what
is in scope. Close the issue when every item is covered, out of scope, or has its own
issue.

A crate release that needs a newer box names that box version in its CHANGELOG
section, and in the README's "Minimum box version" table.

## Prerequisites

- Push access to `lagerdata/lager-rs`.
- A place on the `crates-io` environment's list of required reviewers
  (@danielrmerskine, @adhanali, @cmfisher606). Any one of them can approve the publish.
- For the manual fallback only: owner of `lager-net` on crates.io, and a crates.io API
  token with the `publish-update` scope for `lager-net`.

## Pre-release Checklist

Before you start a release, confirm each item:

- [ ] Every pull request for the release is merged to `main`, and CI on `main` is green.
- [ ] The hardware tests pass on `main` against a real box, if the release changes a
      call to the box. The **Hardware** workflow runs them nightly on the bench box. To
      run it on `main` now, use the command below. To run them against another box, see
      [CONTRIBUTING.md](CONTRIBUTING.md#hardware-tests-require-a-connected-box). Note
      the box software version.
- [ ] The README's "Minimum box version" table and `MISSING_ENDPOINTS.md` describe the
      release.
- [ ] `CHANGELOG.md` has an entry under `## [Unreleased]` for every user-facing change.
- [ ] `## [Unreleased]` holds each `###` heading at most once. Two branches that each add
      a `### Changed` block both merge cleanly. Merge the blocks in Keep a Changelog
      order: Added, Changed, Deprecated, Removed, Fixed, Security. Keep every bullet.
- [ ] No open security issue blocks the release.

This command runs the hardware tests on `main`. The run waits for the bench box when
another run holds it:

```bash
gh workflow run hardware.yml --repo lagerdata/lager-rs --ref main
```

This command counts the `###` headings under `## [Unreleased]`:

```bash
awk '/^## \[Unreleased\]/{f=1;next} /^## \[/{f=0} f' CHANGELOG.md | grep '^### ' | sort | uniq -c
```

## Release Steps

Throughout this guide, replace `X.Y.Z` with the version number, for example `0.8.0`.
See [Versioning Policy](#versioning-policy) to choose it.

### 1. Create a Release Branch

Create the branch from the latest `main`:

```bash
git fetch origin
git switch -c release/vX.Y.Z origin/main
```

### 2. Bump the Version

Set the version in `Cargo.toml`:

```toml
version = "X.Y.Z"
```

Then update the crate's own entry in `Cargo.lock`, and nothing else:

```bash
cargo update -p lager-net
```

On a **minor** release, also update the `version = "X.Y"` dependency lines that tell a
user what to add. There are two in `README.md` (Quickstart and Features) and one in
the crate docs in `src/lib.rs`. A patch release leaves them alone. This command finds
them:

```bash
grep -n 'package = "lager-net", version' README.md src/lib.rs
```

### 3. Update CHANGELOG.md

Keep the `## [Unreleased]` heading. Add the version heading directly under it, above the
entries that ship in this release:

```markdown
## [Unreleased]

## [X.Y.Z] - YYYY-MM-DD
```

A branch that merges after the release files its entry under `[Unreleased]`. Without
that heading, the entry lands inside the released section, and no merge conflict
catches it.

A release with a behavior change, or that needs a newer box, can open its section with
one or two sentences that say so, above the first `###` heading.

### 4. Check the Release Locally

```bash
cargo test --locked --all-features
cargo publish --locked --dry-run
awk -v v="X.Y.Z" 'index($0, "## [" v "]") == 1 {f=1;next} f && /^## \[/{exit} f' CHANGELOG.md
```

The last command prints the release notes exactly as the Release workflow extracts
them.

You can also run the workflow's checks on the branch before you tag anything. A manual
run of the Release workflow runs **verify** only, and never publishes:

```bash
gh workflow run release.yml --repo lagerdata/lager-rs --ref release/vX.Y.Z
```

### 5. Open a Pull Request Against `main`

Commit the changed files, push the branch to `origin` by name, and open a pull request:

```bash
git add Cargo.toml Cargo.lock CHANGELOG.md README.md src/lib.rs
git commit -m "chore: release vX.Y.Z"
git push -u origin release/vX.Y.Z && \
gh pr create --repo lagerdata/lager-rs --base main --head release/vX.Y.Z \
  --title "chore: release vX.Y.Z" --body "Release vX.Y.Z. Notes: the [X.Y.Z] section of CHANGELOG.md."
```

Name the branch in `git push`. A branch that you create from `origin/main` tracks `main`
until its first push.

After CI passes, merge with **Squash and merge**.

### 6. Tag the Release

After the merge, create an annotated tag on the release commit, and push it. Tag
`origin/main` directly, so the state of your local checkout does not matter:

```bash
git fetch origin
git log -1 --format=%s origin/main   # must print: chore: release vX.Y.Z (#N)
git tag -a vX.Y.Z origin/main -m "vX.Y.Z"
git push origin vX.Y.Z
```

Before you tag, confirm that `origin/main` is the release commit. If another pull
request merged after yours, tag the release commit by its SHA instead.

Pushing the tag publishes nothing yet. Until someone approves the deployment in step 7,
you can still delete the tag (see [Troubleshooting](#troubleshooting)).

### 7. Approve the Publish

Open the Release run for the tag:

```bash
gh run list --repo lagerdata/lager-rs --workflow release.yml --limit 1
```

The run shows **verify**, then **publish**, which waits with the status "Waiting". To
approve it:

1. Wait for **verify** to go green. If it fails, do not approve; see
   [Troubleshooting](#troubleshooting).
2. Read the release notes. Download the `release-notes` artifact, or run:

   ```bash
   gh run download <run-id> --repo lagerdata/lager-rs --name release-notes
   ```

3. Open the run's page. In the yellow banner, click **Review deployments**.
4. Select **crates-io**, and click **Approve and deploy**.

The **publish** job then publishes the crate with trusted publishing, and creates the
GitHub Release with the notes as its body. It takes about a minute.

**Approving is the step you cannot undo.** After it, the version is on crates.io for
good. To stop instead, click **Reject**, then delete the tag.

## Post-release

Confirm that the release is live:

- `https://crates.io/crates/lager-net` shows `X.Y.Z`.
- `https://docs.rs/lager-net/X.Y.Z` builds. docs.rs builds with all features enabled,
  as CI's **docs** job does.
- `gh release view vX.Y.Z --repo lagerdata/lager-rs` shows the release, marked
  **Latest**.
- In a scratch crate, `cargo add lager-net@X.Y.Z` resolves and `cargo build` passes.

On a minor release, the docs in [lagerdata/lager](https://github.com/lagerdata/lager)
also pin the crate (`docs/source/reference/rust/`). Open a pull request there to update
them, together with any page that the release changes.

This repository does not delete a merged branch. Delete the release branch:

```bash
git push origin --delete release/vX.Y.Z
git branch -D release/vX.Y.Z
```

## Manual Fallback

Use this only when the Release workflow cannot publish, for example when crates.io
trusted publishing is down. Complete steps 1 through 5 first. Then publish from a clean
checkout of the release commit, and tag only when the publish succeeds:

```bash
git fetch origin
git switch --detach origin/main
cargo publish --locked && \
git tag -a vX.Y.Z -m "vX.Y.Z" && \
git push origin vX.Y.Z
```

Chain the commands. If the publish fails, for example with a `403` from an expired
token, an unchained block still pushes the tag.

When the pushed tag starts the Release workflow, its **publish** job fails, because the
version is already on crates.io. Reject the `crates-io` deployment, and create the
GitHub Release by hand:

```bash
awk -v v="X.Y.Z" 'index($0, "## [" v "]") == 1 {f=1;next} f && /^## \[/{exit} f' CHANGELOG.md > release-notes.md
gh release create vX.Y.Z --repo lagerdata/lager-rs --title "vX.Y.Z" \
  --notes-file release-notes.md --verify-tag
```

## Releasing a Commit That Is Not the Head of `main`

The Release workflow refuses a tag on a commit that is not on `main`. To leave out work
that is already on `main`, revert it on `main` in a pull request, release, and then
revert the revert. Do not release from a side branch.

## Versioning Policy

This project follows [Semantic Versioning](https://semver.org/). Before 1.0, Cargo
treats the **minor** version as the breaking one: a user who depends on `"0.7"` gets
every `0.7.Z`, and never `0.8.0`.

- **MINOR** (`0.Y.0`): New API, support for new box endpoints, and any change that can
  break existing callers. Mark a breaking change in the CHANGELOG as a
  **behavior change**, and say what callers must do.
- **PATCH** (`0.Y.Z`): Bug fixes and documentation that leave every existing call
  working as before.

Each release is a **tag** (`vX.Y.Z`). Do not create a per-version branch.

## Troubleshooting

**verify fails at "Version matches the tag":** The tag and `Cargo.toml` disagree. Delete
the tag, fix the version on `main` in a pull request, and tag again:

```bash
git tag -d vX.Y.Z
git push origin :refs/tags/vX.Y.Z
```

**verify fails at "CHANGELOG has a section for this version":** Step 3 was skipped, or
the heading is not exactly `## [X.Y.Z] - YYYY-MM-DD`. Delete the tag, fix the CHANGELOG
on `main`, and tag again.

**verify fails at `cargo publish --dry-run`:** The tagged commit does not package or
build. Fix it on `main` and tag again. Nothing reached crates.io.

**publish fails after the crate was published:** The version is on crates.io, but the
GitHub Release step did not run. Do not tag again. Create the GitHub Release by hand,
as in [Manual Fallback](#manual-fallback).

**A published version is broken:** Yank it, and release a fixed patch version:

```bash
cargo yank --version X.Y.Z lager-net
```

A yanked version stays downloadable for users whose `Cargo.lock` already pins it, but
no new resolve picks it. Its number can never be published again.

## Backfilling a Missing GitHub Release

Releases before the Release workflow were published by hand, and some of them have a
tag and a crates.io version but no GitHub Release. To add one, extract that version's
CHANGELOG section as above, and create the release on the existing tag. Pass
`--latest=false` for any version older than the current release, so the **Latest**
badge stays on the newest one:

```bash
gh release create vX.Y.Z --repo lagerdata/lager-rs --title "vX.Y.Z" \
  --notes-file release-notes.md --verify-tag --latest=false
```

GitHub dates the release when you create it. The CHANGELOG heading keeps the real
release date.
