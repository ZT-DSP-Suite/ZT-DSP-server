# Pull requests

## Scope

This repository is a **placeholder**. It currently contains licence files, a bilingual
README, and a security policy — no server implementation.

Please do not open implementation pull requests against it yet. The protocol
specification in `ZT-DSP-docs` describes a server-side role, and the implementation is
tracked separately.

## If you are adding the first implementation

The client implementation in `ZT-DSP-client` is the reference for what "correct" means
on the wire. Before writing server code:

- [ ] The Master Guide sections covering the responder role have been read, especially
      the ordering constraints in the Phase 2 section. The responder must receive the
      full frame and complete the Diffie-Hellman computation **before** replying,
      otherwise a low-order-point attacker can distinguish success from failure by timing.
- [ ] Silent-drop behaviour matches the specification: on a parameter mismatch the
      responder must send **nothing at all** on the wire.
- [ ] You have read `SECURITY.md` in this repository. An inconsistency between the
      Chinese and English specification is treated as a **security** matter here, not as
      a formatting matter.