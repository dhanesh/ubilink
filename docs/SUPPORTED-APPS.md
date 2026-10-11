# Supported apps & adding your own (B3)

Live capture (Cmd/Ctrl+Shift+K) works for any app that has a **declarative adapter**.
Adapters are data, not code — adding an app is a JSON entry, no recompile (B3). The
built-ins ship in `core/src/adapters.default.json`; you extend them with a user
file (see below).

## Built-in adapters (seed set)

| App | Mechanism | Link it produces |
| --- | --- | --- |
| Safari | AppleScript (current tab) | the page URL |
| Google Chrome, Arc, Brave Browser, Microsoft Edge, Vivaldi, Opera | AppleScript (active tab) | the page URL |
| Finder | AppleScript (POSIX path) | `file://…` |
| Bear | the open note's id and title from Bear's own CLI, `bearcli app get-selection` (no Accessibility; the clipboard is untouched) | Bear's own id-based link, `bear://x-callback-url/open-note?id=…` |
| Things3 | AppleScript (selected to-do) | `things:///show?id=…` |

These follow patterns ubilink can verify; the broader
list of apps that support ubiquitous linking
(browsers, PDF readers, mail, task/notes, dev tools — ~80 apps) is reachable the same
way: each is a config entry. They are **not** all shipped because each needs the
correct per-app script and testing on a Mac that has the app — work a config entry
makes cheap for you to do locally.

## Add an app yourself — no recompile

Drop an `adapters.json` (a JSON array of specs) in the app data dir:
`~/Library/Application Support/org.dhanesh.ubilink/adapters.json`. Entries
with an `app` matching a built-in **override** it (the result is then a custom
adapter you can switch off, not a built-in); new apps are added. Restart ubilink.

> **Trust:** a custom adapter is trusted local configuration, not a sandboxed
> plugin. Its `script` runs via `osascript` with **your** user privileges and
> Ubilink's Automation/Accessibility grants — AppleScript's `do shell script`
> means it can run any command. Only use scripts you wrote or have read and
> understand. Sharing adapters as copy-paste JSON is not a supported distribution
> channel (see [`DECISIONS.md`](../DECISIONS.md) D14).

### Spec fields
- `app` — the frontmost-app name to match (what `System Events` reports), e.g. `"Notes"`.
- `script` — the `osascript` that reads the active resource.
- `fields` — names bound to the script's output.
- `delimiter` — splits the output into `fields` (omit for a single field).
- `uri`, `name`, `path` — templates over the fields. Transforms: `{x|urlencode}`,
  `{x|basename}`, and `{x|mdurl}` / `{x|mdtext}` (the URL or the text of a Markdown
  `[text](url)` value; a plain value passes through).
- `schemes` — URI schemes the adapter may emit when `uri` has no literal scheme (e.g.
  Bear's `{link|mdurl}` declares `bear`). Declared schemes are added to the link
  allowlist, but a deny-listed scheme (`javascript:`, `data:`, …) is refused anyway.
- `needs_permission` — defaults to `true` (capture waits for the Automation grant).

Every captured link, from any adapter, passes the same validation as the rest of
Ubilink (see [`DECISIONS.md`](../DECISIONS.md) D16).

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
