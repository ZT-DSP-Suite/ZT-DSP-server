# ZT-DSP Server Edition

> **Placeholder repository.** The ZT-DSP server edition has not been implemented yet.
> This repository currently contains only this notice, the bilingual placeholder
> documentation, and the license.

- 中文说明 — [`cn/README.md`](cn/README.md)
- English documentation — [`en/README.md`](en/README.md)

## Why a server edition

The client edition is a direct-connection model: both peers must be reachable. A server
edition is intended to add capability for peers behind NAT, asynchronous delivery,
multi-party groups and identity directories, and connections that do not expose either
peer's IP address — **without weakening the existing end-to-end security properties**.

A server **must not** become a plaintext vantage point: any relay design has to preserve
end-to-end encryption and expose only the minimum metadata needed for routing.

## Status

- Phase: **pre-specification**
- Scope, protocol extensions, threat model and security boundaries will be filled in as the
  design is settled.

## License

Dual-licensed under **MIT OR Apache-2.0** — users may choose either one. See `LICENSE`,
`LICENSE-MIT` and `LICENSE-APACHE`.
