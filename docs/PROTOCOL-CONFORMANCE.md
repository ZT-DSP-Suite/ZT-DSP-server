# Protocol Conformance

This document maps each normative requirement the server must satisfy to the code that
satisfies it. It is written for reviewers: every row should be checkable by reading the
cited file.

Reference specification: **ZT-DSP 1.0.1**, `cn/spec/ZT-DSP-1.0.1-总指导.md`
(English mirror: `en/spec/ZT-DSP-1.0.1-Master_Guide.md`).

---

## 1. Server role

| Requirement (spec) | Where | Implementation |
|---|---|---|
| Server only forwards envelopes; it cannot decrypt | §4.2, line 254 | `src/main.rs` — after pairing the code calls `relay_pair` and never touches the payload |
| Server reads only phase-1 magic routing material | §4.2, line 254 | `src/frame.rs::read_phase1` — extracts the two 32-byte token hashes and nothing else |
| Server must not log pairing information | line 978 (mandatory) | `src/metrics.rs` — counters only, no token hashes, no IPs, no session ids |
| Worst case for an operator is availability, never confidentiality | line 972 | Structural: no key material ever reaches the server |

## 2. Appendix F.0 — framing rules

| Requirement | Implementation |
|---|---|
| TCP byte stream, no message boundaries | `TcpStream` + length-prefixed reads |
| Network byte order throughout | `u32::from_be_bytes` / `u64::from_be_bytes` |
| Variable fields are `[length:4B][payload]` | `src/frame.rs::read_phase1` |
| No magic number, no frame-type field, no frame CRC | The parser assumes position, exactly as specified |
| Validate length *before* reading the payload | `read_phase1` checks each bound, then allocates |

## 3. Appendix F.1 — phase-1 frame

```
[param_len:4][param_json:param_len][identity_len:4][identity_blob][nonce_len:4][nonce]
```

| Check | Code |
|---|---|
| `param_len <= 4096` | `FrameError::LengthExceeded("param_json")` |
| `identity_len <= 8192` | `FrameError::LengthExceeded("identity_blob")` |
| `identity_len >= 64` | `FrameError::IdentityTooShort` |
| `nonce_len <= 512` | `FrameError::LengthExceeded("nonce")` |
| `src = identity_blob[0..32]`, `dst = identity_blob[32..64]` | `Phase1Frame::identity` |
| Default frame length 228 bytes | asserted in `tests::parses_default_phase1` |

The **raw bytes** of the phase-1 frame are preserved verbatim in `Phase1Frame::raw` and
replayed to the peer without re-encoding. The specification requires this because these
bytes feed the phase-3 transcript; any normalisation would break the tag.

## 4. Appendix F.2 — strict silent drop

The contract has five elements, all of which are mandatory:

| # | Element | Implementation |
|---|---|---|
| 1 | Send no bytes | `src/silent.rs` never writes |
| 2 | Do not actively close | The socket is drained until the deadline, then dropped |
| 3 | Drain at 8192-byte chunks until the deadline | `DRAIN_CHUNK = 8192`, loop until `timeout_at` |
| 4 | Diagnostics to local stderr only | `tracing` initialised with `.with_writer(std::io::stderr)` |
| 5 | Indistinguishable from a closed port | Same drain window as a legitimate lobby wait |

Trigger points (each routes to `silent_drop`): parse failure, oversized length, short
identity blob, nonce over cap, and lobby timeout.

## 5. Appendix F.6 — hard limits

| Field | Cap | Code |
|---|---|---|
| Parameter JSON | 4096 | `PARAM_JSON_MAX` |
| Identity declaration | 8192 | `IDENTITY_MAX` |
| Nonce | 512 | `NONCE_MAX` |
| Ciphertext | 1 MiB | `CIPHERTEXT_MAX` |

## 6. §4.2 server processing flow

The specification lists four steps. The server implements steps 1, 3 and 4; step 2 is
not its role.

| Step | Spec text | Implementation |
|---|---|---|
| 1. Parameter check | Reject on shape mismatch | Structural validation in `read_phase1` (lengths); the *peer-to-peer* parameter comparison is not the server's business |
| 2. Token verification | TCA signature + CRL | **Not implemented — belongs to the TCA/CRL role** |
| 3. Lobby matching | "Look for a resident holding H(Token_A)" | `src/lobby.rs::register` |
| 4. Lobby management | 30 s auto-clear, no broadcast | `LOBBY_TIMEOUT`; `withdraw` / `sweep` |

### Pairing predicate

```
newcomer.src == waiter.dst   &&   newcomer.dst == waiter.src
```

Covered by `lobby::tests::mutual_match_pairs_both`, `same_target_without_reverse_match_does_not_pair`
and `impersonation_rejected`.

## 7. Appendix B.2 — profiles

| Profile | Concurrent | Per-IP | Offline | Groups | Directory | Idle timeout |
|---|---|---|---|---|---|---|
| `beginner` | 2000 | 32 | 1 day | 32 members | on | none |
| `advanced` | 20000 | 128 | 7 days | 256 members | on | none |
| `high-anonymity` | 500 | 4 | **disabled** | **disabled** | **disabled** | 300 s |

The `high-anonymity` tier disables every store. This is asserted by
`config::tests::high_anonymity_disables_storage_features`.

## 8. Deliberately out of scope

| Item | Reason |
|---|---|
| TCA token issuance | Separate authority with its own keys and audit trail |
| CRL distribution and revocation checking | Same |
| Phase-2/3/4 cryptographic processing | The server must not hold session keys |
| Persistence of offline envelopes | Deployment concern; in-memory is the safe default |
| Authenticated client identity at the server | Would require the server to know who is who, which defeats the anonymity model |

## 9. Where the specification is silent

The following behaviours are **not defined** by ZT-DSP 1.0.1 and are implemented here as
documented extensions. They never affect the data plane.

- Offline envelope queueing (`src/l4.rs::OfflineStore`)
- Group log with per-member cursors (`src/l4.rs::Groups`)
- Identity directory (`src/l4.rs::Directory`)

See [`CONTROL-API.md`](CONTROL-API.md).
