# Runbook — integration registry & review process (O4)

O4 wants a documented path to contribute integrations and to publish/review them via
a registry, enabling the B3 ecosystem. The registry **service** is built
(`registry/`, std-only HTTP, reuses ubilink-core's ed25519 trust) and verified
headlessly — both by unit tests and a real-socket round trip. What stays ENV-blocked
is *operating a hosted, governed instance* (a public host, TLS, persistence, and an
actual review team).

## What is already proven (no hosting needed)
- `RegistryIndex`: publish verifies the bundle against trusted publishers, enforces
  the manifest schema, binds each id to its first publisher key, and enforces
  version monotonicity; get / list / search work.
- `handle_request` + `handle_stream`: the full HTTP API, tested in-memory.
- The real binary `ubilink-registry` over a live local socket: `POST /publish` (201),
  `GET /integrations` (+ `?q=` search), `GET /integrations/{id}` (verbatim bundle),
  untrusted publish → 400, unknown id → 404.
- `serve`: timeouts, request deadline and the connection bound are tested over real
  loopback sockets (an idle client does not block others; over-limit → 503).

## What a publish must pass
1. **Signature + trusted publisher** — ed25519 over the manifest, version and source,
   by a key in `UBILINK_TRUSTED_KEYS`.
2. **Manifest schema** — `schema/integration.manifest.schema.json`, enforced by
   `Manifest::validate` inside `Installer::verify` (the same gate the on-device
   installer and `ubilink-sign verify` use): reverse-DNS `id`, semver `version`,
   `verb:scope` needs with a lowercase verb, no duplicate needs, no unknown fields.
3. **Id ownership** — the first key to publish an id owns it; a bundle for that id
   signed by any other key (even a trusted one) is refused with 400. **Key
   rotation / id transfer is not supported yet**: an owner who loses or rotates
   their key cannot publish that id again without operator intervention.
4. **Monotonic version** — the bundle's integer `version` (the `--version` given to
   `ubilink-sign sign`) must be strictly greater than the published one.

## Request limits (DoS bounds)
| Bound | Value | Answer when exceeded |
| --- | --- | --- |
| Request body (`Content-Length`) | 1 MiB — bundles are a few KiB | `413`, before allocating |
| Request line | 8 KiB | `414` |
| Header section | 16 KiB total | `431` |
| Concurrent connections | 64 | `503` from the accept loop |
| Per read/write wait | 10 s | connection closed |
| Whole request (and, separately, the response) | 30 s | connection closed |

Chunked `Transfer-Encoding` is refused (`501`); `POST` without `Content-Length` is
`411`. The index lock is taken only to apply an already-parsed request, never
during socket I/O. Each connection serves one request (`Connection: close`).
These are process-wide bounds, not per-client: one client can still take up to
all 64 slots for up to 30 s each — put a proxy with per-IP limits in front of a
public instance.

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
   to the registry's `UBILINK_TRUSTED_KEYS`. Only then can they publish; revoke trust
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
UBILINK_TRUSTED_KEYS=<comma-separated approved pubkeys> \
  cargo run -q --bin ubilink-registry -- 0.0.0.0:8787
```

## Deployment residual (ENV-blocked — what flips O4 → SATISFIED)
- A reachable host + TLS terminator in front of the service.
- Durable storage (the index is in-memory today; back it with a file/db on publish).
  Persist each id's owner key alongside its bundle: while the index is in-memory,
  a restart forgets ownership and the first trusted publish after it claims the id.
- A standing maintainer team actually performing reviews.
Record O4 → SATISFIED only once a governed instance is operating — not from the
headless build, which proves the mechanism, not the running institution.
