# ZT-DSP Server Edition (work in progress)

## 这里有什么

The English description of the relay server.

## 按什么找

Protocol behaviour is specified in ZT-DSP-docs.

> This repository is a **placeholder** for the ZT-DSP server edition. No implementation has started yet.

## Why a server edition is needed

The current client edition (repository `ZT-DSP-client`) is a direct-connection model: both peers must be reachable — **each side needs a public IP address or another direct path** (port forwarding, IPv6, a tunnel, etc.). That works for small, mutually trusting groups, but not for:

- a peer behind NAT that cannot forward ports;
- asynchronous delivery (messages when the other side is offline);
- multi-party groups, identity directories, or cross-organization federation;
- establishing a connection without exposing either peer's IP address.

The server edition aims to add these capabilities **without weakening the existing end-to-end security properties**, while remaining compatible and interoperable with the client protocol.

## Status

- Phase: **pre-specification**
- This repository currently contains only this placeholder notice and the license. Scope, protocol extensions, threat model, and security boundaries will be filled in as the design is settled.
- A server **must not** become a plaintext vantage point: any relay design must preserve end-to-end encryption, and the server should see only the minimum metadata needed for routing.

## Versions

| Item | Value |
|------|-------|
| applicable total-guide version | 1.0.0 (may be several; see "Applicability" below) |
| client version | ZT-DSP_PY-1.0.0 (repository `ZT-DSP-client`), ZT-DSP_RS-1.0.0 (same repository) |
| server version | ZT-DSP_SV-1.0.0 (this repository, placeholder, not implemented yet) |
| followed document | "ZT-DSP Protocol Master Guide", path ZT-DSP-docs/en/spec/ZT-DSP-1.0.1-Master_Guide.md |

### Applicability (many-to-many)

The relation between a total-guide version and the client/server versions is **not** one-to-one; it is many-to-many:

| Relation | Rule |
|----------|------|
| one total-guide version -> several implementation versions | several implementations of the same kind whose protocol behaviour has not changed **share one declaration**; no rewrite is needed per version |
| several total-guide versions -> one implementation version | one implementation may apply to several guides at once; listed item by item in `APPLIES_TO_DOC_VERSIONS` |
| same number, different implementation | identical version numbers do **not** mean the same implementation and do not constitute a compatibility claim |
| how compatibility is decided | applicability is a **declaration**, not a test; real compatibility is always shown by the phase-three confirmation-label check |

Handshake frames carry **no version string**: neither the phase-one parameter JSON nor the phase-four AAD
carries a version or protocol identifier field.

> This repository has no implementation yet. `ZT-DSP_SV-1.0.0` above is a **placeholder identifier** reserved
> to carry the applicability relation; it is not a released implementation.

## Layout

- `cn/` — Chinese documentation
- `en/` — English documentation

## Licence

Dual-licensed under **AGPL-3.0-or-later** or a **commercial license**. See [LICENSING.md](../LICENSING.md) and [LICENSE](../LICENSE).
