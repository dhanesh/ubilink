# graph-data: Export and import round-trip the graph; Reset asks first

- id: graph-data
- proven: no
- anchors: src/views/settings.rs:SettingsView, probes/graph-roundtrip.json, probes/graph-reset.json, probes/fixtures/graph-backlinks.json, probes/fixtures/graph-empty.json
- constraints: linking-manifesto-app:T2

## What it is

Settings can export the whole graph to a JSON file and import it back with no
change (Phase 1 R11), and can reset the graph to empty — but only after an explicit
confirmation step, never on the first press (Phase 1 R12). A stats line
(`"<resources> link(s) · <links> connection(s)"`) is re-read every time Settings is
shown, so it reflects whatever the graph holds right now, in-process edit or
out-of-band seed alike.

## How to reach it

- **Tab to Export/Import/Reset, then Enter** (`step:{"key_until": {...}}`, then
  `key {"keys": "enter"}`).
- **Not driven here:** the launch-at-login switch (`launch-at-login.md`), adapter
  switches (`settings-adapters.md`) — same view, different controls.

## Drive it

```sh
OUT="$WT/.verify-run/$INSTANCE/captures/graph-data"
python3 "$H" call --instance "$INSTANCE" --verifier "$VERIFIER" --out "$OUT" \
  'seed_graph:{"fixture":"graph-backlinks.json"}' \
  'open_view:{"route":"settings"}' \
  'wait_state:{"shell.visible":true,"route":"settings"}' \
  'see:{"role":"Label","text":"5 link(s) · 3 connection(s)","exact":true}' \
  'step:{"key_until":{"role":"Button","text":"Export","state":{"focused":true}},"key":"tab"}' \
  'key:{"keys":"enter"}' \
  'step:{"graph":true,"export":true,"link_count":3,"resource_count":5}' \
  'step:{"key_until":{"role":"Button","text":"Reset","state":{"focused":true}},"key":"tab"}' \
  'key:{"keys":"enter"}' \
  'see:{"role":"Label","text":"Delete all captured links and connections?"}' \
  'step:{"key_until":{"role":"Button","text":"Delete everything","state":{"focused":true}},"key":"tab"}' \
  'key:{"keys":"enter"}' \
  'step:{"graph":true,"resource_count":0,"link_count":0}' \
  'see:{"role":"Label","text":"0 link(s) · 0 connection(s)","exact":true}' \
  'step:{"key_until":{"role":"Button","text":"Import","state":{"focused":true}},"key":"tab"}' \
  'key:{"keys":"enter"}' \
  'step:{"graph":true,"resource_count":5,"link_count":3}' \
  'see:{"role":"Label","text":"5 link(s) · 3 connection(s)","exact":true}' \
  opened
echo "exit $?"
```

Expected: every step prints `"ok": true`, exit 0; `opened` reports `"result": []`.
The scenario's own `export_path`/`import_path` fixture binding (same file both ways,
no dialog, per probes/README.md's `setup.fixtures`) is set at launch, outside this
`call`; `probes/graph-roundtrip.json` seeds the empty-graph fixture between export
and import with `seed_graph ... replace: true` rather than Reset, so this map's use
of Reset to empty the graph before Import is a deliberate substitution that also
exercises R12 in the same pass.

## Proof

- **Export preserves the link set.** Reading the export path back
  (`step:{"graph": true, "export": true}`) shows the same resource/link counts the
  live graph had.
- **Reset asks first.** No confirmation `Label`/buttons exist before Reset is
  pressed; pressing it shows the question and both buttons, and the graph is
  unchanged until "Delete everything" is actually pressed — a Cancel path (not driven
  in this command; see `probes/graph-reset.json`) must leave the graph untouched too.
- **No role asserted on the confirmation surface itself** — gpui-component's
  `Dialog` container never reaches AccessKit in this gpui-pre version
  (A11Y-NOTES.md); the proof is the question text and the two buttons, not a
  `Dialog` role.
- **Import restores exactly.** After the graph is emptied, Import brings the counts
  and the stats line back to the original.
- **Stats re-read every show.** The Label matches the Tauri wording exactly
  (`"<n> link(s) · <n> connection(s)"`) and is asserted fresh after every graph
  change in this command, proving Settings does not cache it.
- **Side effect.** The `opened` sink stays `[]`: export/import/reset never open
  anything.

## Gotchas

- Import **merges** into the live graph (`LinkGraph::import_json`'s own semantics);
  emptying it first (Reset here, `seed_graph ... replace: true` in the Phase 1
  scenario) is required before Import can be read as "restores exactly," not just
  "adds more."
- `graph-empty.json` alone, imported without first clearing, would change nothing —
  it is not itself a reset.
- Today's stub has no stats Label and no Export/Import/Reset buttons at all, so this
  call fails at the first `see` until Phase 1 lands.
