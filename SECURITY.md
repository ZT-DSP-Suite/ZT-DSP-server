# Security Policy

## Supported versions

This repository is a **placeholder**: it contains no server implementation yet (phase: pre-specification).
There is therefore no deployed server surface to report a vulnerability against.

| Version | Supported |
|---|---|
| `ZT-DSP_SV-1.0.0` (placeholder identifier) | No -- not an implementation |

## Reporting a vulnerability

Once a server implementation exists, vulnerabilities are reported through **private vulnerability reporting**:

1. Open the repository on GitHub.
2. Go to **Security** -> **Advisories** -> **Report a vulnerability**.

> That channel only works after **Settings -> Code security -> Enable private vulnerability reporting** is turned on.
> It is **off by default** on a new repository. Until it is enabled, open a public issue that contains no exploit
> details, and the maintainer will move the discussion to a private channel.

## What counts as in scope

- **In scope**: the implemented server code, its build and release pipeline, its configuration and CI,
  and protocol-document errors that would make the two client implementations diverge.
- **Out of scope**: unimplemented design ideas, documentation wording, non-security feature requests.
- **Mirror consistency is in scope**: if `cn/` and `en/` disagree and the disagreement touches a
  security statement (for example a restriction stated in only one language), report it as a security
  issue rather than a documentation bug.

## Disclosure timeline

Coordinated disclosure through the private channel. No response-time commitment is published for this
placeholder repository.
