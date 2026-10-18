# Changelog

All notable changes to this repository are recorded here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

**Note on authority:** this file is a convenience record, not the source of truth. The
governing change history is `git log`. If the two ever disagree, `git log` wins.

## [Unreleased]

### Added

- **Server implementation (Rust).** Data plane implementing ZT-DSP 1.0.1:
  - Appendix F.1 phase-1 frame parsing with F.6 length limits enforced before allocation.
  - Appendix F.2 strict silent drop: no bytes written, no active close, 8192-byte drain,
    diagnostics on stderr only, indistinguishable from a closed port.
  - §4.2 lobby pairing with mutual-match predicate and a 30 s timeout (Appendix H.3).
  - Bidirectional byte relay that never re-encodes or inspects phase 2–4.
  - Appendix B.2 profile tiers (`beginner`, `advanced`, `high-anonymity`).
- **L4 control plane** (documented extension, separate port, JSON Lines):
  - offline envelope queue with FIFO eviction and TTL,
  - group log with per-member cursors,
  - identity directory with minute-coarsened timestamps and arbitrary lookup off by default.
- `docs/ARCHITECTURE.md`, `docs/PROTOCOL-CONFORMANCE.md`, `docs/CONTROL-API.md`,
  `docs/THREAT-MODEL.md`, `docs/DEPLOYMENT.md`.
- End-to-end and L3/L4 test scenarios under `tests/`.

### Changed

- **Relay return type no longer lies**: relay_pair never produced an Err, so its
  io::Result wrapper was dead ceremony. It now returns the tuple directly.
- **L4Error is a real error type**: implements std::error::Error and PartialEq,
  and carries a stable machine-readable code(). The control plane now answers with
  error_code next to the human-readable error text, so machine clients no longer
  have to parse prose.
- **Mutex poisoning no longer panics a connection**: all 17 expect sites go through
  a lock() helper that recovers the inner value. Every locked section is panic-free,
  so poisoning can only come from a cancelled task, and the counters and maps are
  still self-consistent.
- **write_frame has a write timeout**, so the phase-1 replay during a pairing
  handoff cannot hang a connection forever.
- **Configuration is validated at startup** instead of being silently clamped at
  runtime: the nine .max(1) clamps are gone, and a zero value now stops the process
  from listening. Values belonging to a disabled feature are not checked.
- **The accept loop backs off** on error instead of retrying at full speed.
- **ct_eq compares length in constant time too** - the length difference no longer
  short-circuits the loop.
- **read_line_limited fills the buffer once**; the previous two-call pattern could
  desynchronise the chunk length if the buffer was rewritten in between.
- **A failed response serialisation is no longer swallowed** into a fake ok:false
  body.
- **The control plane rejects unknown fields.**
- **The profile serialised name has a single source of truth** (serde kebab-case);
  the duplicate as_str is gone and a test pins the wire names.
- **A redundant guard arm** in the frame reader was removed, and a stale
  allow(dead_code) on a field that is read was dropped.
- **THIRD-PARTY-NOTICES.md** lists all 57 dependencies with the licence taken from
  each pinned version's own Cargo.toml.
- **test-vectors/** reuses the client repository's shared vectors, with a test that
  keeps the -v3 derivation-label convention in lockstep with the client.

- **Source consolidated into a single file** (`src/main.rs`), per the collection rule in
  `ZT-DSP-client/cn/README.md`: one implementation is one file, and the mainline directory
  does not grow a directory tree. Module boundaries are kept as inline `mod` blocks, and
  their `pub` surface is narrowed accordingly.
- **Version contract declared** in `src/main.rs` (`mod version`): `FOLLOWS_DOC`,
  `FOLLOWS_DOC_PATH`, `APPLIES_TO_DOC_VERSIONS`, plus `IMPLEMENTATION_ID` and
  `format_version()`. The same shape as `zt_dsp_py/version.py` and `zt_dsp_rs/version.rs`,
  with a role prefix (`SV`) instead of a language prefix. `--version` and the startup
  banner read from it.
- **CI added** (`.github/workflows/ci.yml`): a matrix of native `ubuntu-24.04` and
  `ubuntu-24.04-arm` runners, each building its own target, running fmt, clippy, tests,
  a release build, and asserting the produced binary's `readelf` machine matches. This is
  what makes the "both targets build" claim checkable; actions are pinned to commit SHAs
  and no secrets are referenced.
- `scripts/check.sh` gained two checks: `src/` must contain exactly one `.rs` file, and
  the version contract symbols must be present.
- `README.md`, `cn/README.md`, `en/README.md` rewritten from placeholder text to describe
  the implemented server, with a conformance table pointing at the specification.
- `scripts/check.sh` rewritten: it previously rejected any `.rs`/`.toml`/`.py` file and
  demanded placeholder wording in the README, which is no longer true now that an
  implementation exists. It now checks licence headers and mirror consistency instead.
- `SECURITY.md` updated: there is now a deployed surface to report against.

### Removed

- The "placeholder repository" notice from all three READMEs.

### Notes

- TCA token issuance and CRL distribution remain out of scope: they are a separate
  authority, not part of the server role.

## [1.0.0]

- Initial placeholder: licence files and bilingual README.
