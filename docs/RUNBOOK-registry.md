# Runbook — integration registry & review process (O4)

O4 wants a documented path to contribute integrations and to publish/review them via
a registry, enabling the B3 ecosystem. The registry **service** is built
(`registry/`, std-only HTTP, reuses ubilink-core's ed25519 trust) and verified
headlessly — both by unit tests and a real-socket round trip — and persists its
index to a data directory. What stays ENV-blocked is *operating a hosted, governed
instance* (a public host, TLS, backups, and an actual review team).

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
- Persistence: entries and owners survive a reload (and a `serve` restart over HTTP),
  a second trusted key still cannot take over an id after a restart, and a failed
  write fails the publish without changing the index. See [Data directory](#data-directory-persistence).

## What a publish must pass
1. **Signature + trusted publisher** — ed25519 over the manifest, version and source,
   by a key in `UBILINK_TRUSTED_KEYS`.
2. **Manifest schema** — `schema/integration.manifest.schema.json`, enforced by
   `Manifest::validate` inside `Installer::verify` (the same gate the on-device
   installer and `ubilink-sign verify` use): reverse-DNS `id`, semver `version`,
   `verb:scope` needs with a lowercase verb, no duplicate needs, no unknown fields.
3. **Id ownership** — the first key to publish an id owns it; a bundle for that id
   signed by any other key (even a trusted one) is refused with 400. With a data
   directory, ownership survives restarts. **Key rotation / id transfer is not
   supported yet**: an owner who loses or rotates their key cannot publish that id
   again without operator intervention.
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
UBILINK_REGISTRY_DATA=/var/lib/ubilink-registry \
  cargo run -q --bin ubilink-registry -- 0.0.0.0:8787
```
Always set `UBILINK_REGISTRY_DATA` for a real instance. Without it the index is in
memory only, and the binary warns at startup: a restart forgets every integration
**and its owner**, so the first trusted publish after it can claim any id.

## Data directory (persistence)
`UBILINK_REGISTRY_DATA` names a directory, which is created if missing. Tests and
embedders use `RegistryIndex::open(trusted_keys, dir)`; `RegistryIndex::new` stays
purely in memory.

**Layout.** One file per integration id:
```
<data-dir>/
  .lock                   single-writer lock (held while the registry runs)
  com.example.app.json    {"format": 1, "owner_hex": "<64 hex>", "wire_json": "<signed bundle>"}
  com.example.app.json.tmp   only while a write is in flight, or after a crash mid-write
```
`wire_json` is the exact published bundle, served back verbatim. `owner_hex` is the
key that owns the id. Ids are lowercase reverse-DNS names, so they make safe file
names; the store re-checks the pattern and refuses `/`, `\` and `..` before building
a path.

**Publishing.** An accepted publish is written *before* the index changes and before
`201` is sent. The write goes to `<id>.json.tmp`, is fsynced (`F_FULLFSYNC` on
macOS), is renamed over `<id>.json`, and then the directory is fsynced. A reader or
a crash therefore sees the whole old record or the whole new one. If the write
fails, the publish gets `500` and the index is unchanged. If only the final
directory fsync fails, the new record may already be on disk while memory still has
the old one. The registry then refuses **every** further publish until it is
restarted, and the restart reloads from disk. The write happens under the index
lock, so the ownership check, the write and the in-memory update are one atomic
step.

**One registry per directory.** A second process (or a second index in the same
process) that opens a directory already in use fails at startup with "already in
use". Two writers would each decide ownership from their own memory.

**Startup load.** Every `<id>.json` is re-verified with the same gate as a publish:
signature, a key in the *current* `UBILINK_TRUSTED_KEYS`, and the manifest schema.
The bundle must also be for the file's id and signed by the recorded owner. The log
line `loaded N integration(s), M reserved` summarises the result. Then:
- `*.tmp` files are ignored. They are writes that never completed and were never
  acknowledged; the previous record is intact. The next publish of that id
  replaces them. Dotfiles and anything not named `<id>.json` are ignored with a
  warning.
- A record that fails (its key was removed from trust, or the file was edited,
  truncated, or holds another id's bundle) is **not served**. Its id stays
  **reserved**: *no* key can publish it, not even the recorded owner, and the
  publish error says so. This fails closed on purpose. Once a record fails, its
  owner and version cannot be trusted, and freeing the id would let another trusted
  key claim it. A warning names the file and the reason.

**Clearing a reserved id.** Records are read only at startup, so stop the registry
first, act, then start it again.
- Trust removed by mistake: re-add the key to `UBILINK_TRUSTED_KEYS`.
- Corrupt or tampered file: restore that file from a backup.
- Id should be free for someone else (a deliberate revocation or transfer): move
  the file out of the directory. The next trusted publish then claims the id, so
  this is the operator's manual stand-in for key rotation, which is not supported.
Never edit a record while the registry is running; the next publish of that id
overwrites it.

**Backup and restore.** Every record is replaced atomically, so copying the
directory while the registry runs (`cp -a`, `rsync`, `tar`) yields whole records.
Ids are independent, so there is no cross-file consistency to preserve. Skip `.lock`
and `*.tmp`. To restore, stop the registry, replace the directory's contents, and
start it; every record is re-verified on load. A backup rolls those ids back to
their backed-up versions and owners. Ids first published after the backup are gone
and can be claimed again, so back up after every publish or on a short schedule.

## Deployment residual (ENV-blocked — what flips O4 → SATISFIED)
- A reachable host + TLS terminator in front of the service.
- Durable, backed-up storage for `UBILINK_REGISTRY_DATA` (a persistent volume plus
  a backup schedule; see above).
- A standing maintainer team actually performing reviews.
Record O4 → SATISFIED only once a governed instance is operating — not from the
headless build, which proves the mechanism, not the running institution.
