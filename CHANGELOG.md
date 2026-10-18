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
