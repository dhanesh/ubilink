# Runbook — integration registry & review process (O4)

O4 wants a documented path to contribute integrations and to publish/review them via
a registry, enabling the B3 ecosystem. The registry **service** is built
(`registry/`, std-only HTTP, reuses hooky-core's ed25519 trust) and verified
headlessly — both by unit tests and a real-socket round trip. What stays ENV-blocked
is *operating a hosted, governed instance* (a public host, TLS, persistence, and an
actual review team).

## What is already proven (no hosting needed)
- `RegistryIndex`: publish verifies the bundle against trusted publishers and
  enforces version monotonicity; get / list / search work (6 lib tests).
- `handle_request` + `handle_stream`: the full HTTP API, tested in-memory.
- The real binary `hooky-registry` over a live local socket: `POST /publish` (201),
  `GET /integrations` (+ `?q=` search), `GET /integrations/{id}` (verbatim bundle),
  untrusted publish → 400, unknown id → 404.

## The review process (the trust gate IS the review)

A publish is accepted only if the bundle is signed by a key in the registry's
**trusted publisher set**. That set is the governance lever:

1. **Contribute.** An author builds + validates + signs their bundle following
   `docs/AUTHORING-INTEGRATIONS.md` and opens a submission (PR with the manifest +
   `plugin.mjs`, or a bundle + publisher pubkey).
2. **Review.** A maintainer reviews the manifest (least-privilege `needs[]`, no
   attempt at `read:link-graph`) and the plugin source (what it does with the
   capabilities). Use the criteria below.
3. **Approve = trust.** On approval, the author's publisher **public key** is added
   to the registry's `HOOKY_TRUSTED_KEYS`. Only then can they publish; revoke trust
   by removing the key. Review is enforced cryptographically, not by convention.
4. **Publish.** The signed bundle is `POST`ed to `/publish`; users discover it via
   `/integrations` and install the verbatim signed bundle (the `Installer` re-verifies
   on-device — defence in depth).

### Review criteria
- `needs[]` is least-privilege and uses only the documented `verb:scope` grammar.
- No capability that could exfiltrate the graph (the broker denies these anyway, S1).
- `plugin.mjs` does only what its capabilities imply; no obfuscation.
- `version` increases monotonically across submissions.

## Running the registry
```sh
HOOKY_TRUSTED_KEYS=<comma-separated approved pubkeys> \
  cargo run -q --bin hooky-registry -- 0.0.0.0:8787
```

## Deployment residual (ENV-blocked — what flips O4 → SATISFIED)
- A reachable host + TLS terminator in front of the service.
- Durable storage (the index is in-memory today; back it with a file/db on publish).
- A standing maintainer team actually performing reviews.
Record O4 → SATISFIED only once a governed instance is operating — not from the
headless build, which proves the mechanism, not the running institution.
