# Security policy

## Supported versions

Only the **latest release** of Ubilink receives security fixes.

## Reporting a vulnerability

Please report vulnerabilities **privately** through GitHub:
**Security → Report a vulnerability** on
[github.com/dhanesh/ubilink](https://github.com/dhanesh/ubilink/security/advisories/new).
Don't open a public issue for a security problem.

Include what you found, how to reproduce it, and the Ubilink version (shown in
Settings). You'll get an acknowledgement, and a fix or a decision, as soon as the
maintainer can; reporters are credited in the advisory unless they ask otherwise.

## What's in scope

- The macOS app: link capture, validation and opening, import/export, the local
  database, and the permission model.
- The release pipeline and signed artifacts.
- The developer-facing integration sandbox, broker and registry (not yet enabled in
  the app).

How Ubilink thinks about these boundaries is described in the README's
[Security model](README.md#security-model-why-plugins-cant-hurt-you) and in
[`DECISIONS.md`](DECISIONS.md) (D14: custom adapters are trusted local config; D16:
one link-validation policy).
