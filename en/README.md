# ZT-DSP Server Edition (work in progress)

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

## Layout

- `cn/` — Chinese documentation
- `en/` — English documentation

## Licence

This project is licensed under **MIT** and **Apache-2.0**; a user may choose either one. See `LICENSE`, `LICENSE-MIT` and `LICENSE-APACHE`.
