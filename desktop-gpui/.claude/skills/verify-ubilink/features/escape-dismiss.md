# escape-dismiss: Escape backs out one step, then dismisses the panel

- id: escape-dismiss
- proven: no
- anchors: src/views/escape.rs, probes/escape-dismiss.json

## What it is

Escape works as it did in the Tauri panel (`ee80e70:dist/panel.js` 555-592):

- **Capture and onboarding:** Escape hides the panel.
- **Browser:** Escape cancels a pending ⌘L mark. With no mark, it clears a non-empty
  filter. With neither, it hides the panel.
- **Settings:** Escape closes an open reset question without deleting anything.
  Otherwise it hides the panel.

The panel hides through the shell's `Command::Hide`, so it really leaves the screen.
A hands-on check of the shipped app found GPUI handled none of this (Phase 2b
regression 1).

## How to reach it

- **`key {"keys": "escape"}`** sends a real Escape keystroke through GPUI's keymap and
  listeners in the panel window. In the browser, the focused filter Input maps it to
  its `Escape` action; in the other views, the view's key listener receives it.
- The browser's mark is `cmd-l` (`link-marking.md`), and its filter text is typed
  keys.
- The reset question is reached with Tab to `Reset…`, then Enter (`graph-data.md`).

## Drive it

```sh
OUT="$WT/.verify-run/$INSTANCE/scenarios"
python3 "$H" scenario --instance "$INSTANCE" --verifier "$VERIFIER" --out "$OUT" --name escape-dismiss
echo "exit $?"
```

Expected: exit 0, and `verdict` `"PASS"` in `$OUT/escape-dismiss/result.json`.
`transcript.txt` should show these steps PASS:

- For each route: "<route>: Escape dismisses the panel", "<route>: shell has no
  panel" and "<route>: window left the screen".
- Browser: "browser: Escape cancelled the mark", "browser: Escape cleared the filter"
  and "browser: the rows are unfiltered again".
- Settings: "settings: Escape closed the confirmation" and "settings: nothing was
  deleted".

## Proof

- **Dismiss.** For every route, three checks agree:
  - `state.shell.visible` is false and `route` is null;
  - `window_id` refuses;
  - the window is gone from Quartz's on-screen list (`winframe: gone`).
- **Browser order.** The mark goes first (its `Linking from` Label is gone). The
  filter goes next: it no longer reads `be`, and the rows are the seed's top-level
  order again. Only the third Escape hides the panel.
- **Settings order.** The question closes, `graph` still counts 5 resources and 3
  links, and only the next Escape hides the panel.
- **Side effect.** "nothing launched" requires the `opened` sink to equal `[]`.

Record it (SKILL.md, Evidence):

```sh
python3 "$H" doctor --instance "$INSTANCE"
R="$OUT/escape-dismiss"
RESULT=$(python3 -c 'import json,sys; r=json.load(open(sys.argv[1])); print("pass" if r["exit"]==0 else "fail")' "$R/result.json")
python3 "$SKILL"/scripts/verify_evidence.py record --worktree "$WT" --instance "$INSTANCE" --feature escape-dismiss \
  --verifier "$VERIFIER" --result "$RESULT" \
  --action "scenario escape-dismiss (desktop-gpui/probes/escape-dismiss.json) on instance $INSTANCE" \
  --observed "$(python3 -c 'import json,sys; r=json.load(open(sys.argv[1])); print(r["verdict"], r["counts"])' "$R/result.json")" \
  --side-effect "opened sink stayed []; graph reseeded from graph-backlinks.json after the capture block; see result.json" \
  $(for f in "$R"/*; do printf -- '--artifact %s ' "$f"; done)
```

## Gotchas

- The landed capture stores `https://example.com/`, so the scenario reseeds the
  graph (`seed_graph replace`) before the browser block. Otherwise the browser's row
  order and the settings block's counts would include that capture.
- At 4c6e66d, a real Escape on the live capture panel still closed it. The `key`
  verb's GPUI-dispatched Escape did not: that step fails at 4c6e66d. The live close
  most likely came from AppKit (unconfirmed): GPUI left Escape unhandled, so AppKit's
  own handling ran outside the shell. Now GPUI handles Escape and stops it there.
- Hiding is deferred until the key's dispatch is over, because the shell removes the
  very window that is dispatching it. Read `visible` with `retry_ms`.
