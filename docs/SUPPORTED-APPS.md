# Supported apps & adding your own (B3)

Live capture (Cmd/Ctrl+Shift+K) works for any app that has a **declarative adapter**.
Adapters are data, not code — adding an app is a JSON entry, no recompile (B3). The
built-ins ship in `src-tauri/src/adapters.default.json`; you extend them with a user
file (see below).

## Built-in adapters (seed set)

| App | Mechanism | Link it produces |
| --- | --- | --- |
| Safari | AppleScript (current tab) | the page URL |
| Google Chrome, Arc, Brave Browser, Microsoft Edge, Vivaldi, Opera | AppleScript (active tab) | the page URL |
| Finder | AppleScript (POSIX path) | `file://…` |
| Bear | window title via System Events | `bear://x-callback-url/open-note?title=…` |
| Things3 | AppleScript (selected to-do) | `things:///show?id=…` |

These follow patterns ubilink can verify; the broader
list of apps that support ubiquitous linking
(browsers, PDF readers, mail, task/notes, dev tools — ~80 apps) is reachable the same
way: each is a config entry. They are **not** all shipped because each needs the
correct per-app script and testing on a Mac that has the app — exactly the work a
config entry makes cheap and shareable.

## Add an app yourself — no recompile

Drop an `adapters.json` (a JSON array of specs) in the app data dir:
`~/Library/Application Support/org.dhanesh.ubilink/adapters.json`. Entries
with an `app` matching a built-in **override** it; new apps are added. Restart ubilink.

### Spec fields
- `app` — the frontmost-app name to match (what `System Events` reports), e.g. `"Notes"`.
- `script` — the `osascript` that reads the active resource.
- `fields` — names bound to the script's output.
- `delimiter` — splits the output into `fields` (omit for a single field).
- `uri`, `name`, `path` — templates over the fields. Transforms: `{x|urlencode}`, `{x|basename}`.

### Example — Apple Notes
```json
[
  {
    "app": "Notes",
    "script": "tell application \"Notes\" to return (id of item 1 of (selection)) & \"\\t\" & (name of item 1 of (selection))",
    "delimiter": "\t",
    "fields": ["id", "title"],
    "uri": "notes://showNote?identifier={id}",
    "name": "{title}"
  }
]
```

If the app you want has no AppleScript/URL-scheme support, capture isn't possible —
ubilink says so honestly (U3) rather than producing a dead link.
