# gpui-component 0.7.0 accessibility notes (Task 7, Phase 0 R7/TN5)

Task 6 proved a stub view's `text!` title reaches gpui's AccessKit tree as a `Label`
node, and that a plain `&str` child does not (GPUI only pushes an a11y node for an
element that has both an `.id(...)` and a non-`GenericContainer` `.role(...)` —
gpui-pre 0.3.7's `element.rs`, `ElementDrawPhase::prepaint`). This task checked
gpui-component 0.7.0's `List`, `Input`, `Switch` and `Dialog` the same way, using the
automation `a11y` verb against a live, isolated instance
(`desktop-gpui/probes/a11y-spike.json`, 11/11 steps pass). The spike view is
`desktop-gpui/src/views/a11y_spike.rs` (automation builds only).

Verdict up front: **List, Input and Switch reach the tree correctly with their
plain, documented API — no workaround needed.** Dialog's own container role does
**not** reach the tree (a real gpui-pre 0.3.7 gap, detailed below), but its title and
content text do, through the same `text!` pattern as everything else.

## List (`gpui_component::list::{List, ListState, ListDelegate, ListItem}`)

Raw dump excerpt (three rows, "alpha" selected):

```
"List: "
"ListItem:  {'selected': True, 'position_in_set': 1, 'size_of_set': 3}"
"Label: Spike row alpha"
"ListItem:  {'position_in_set': 2, 'size_of_set': 3}"
"Label: Spike row beta"
"ListItem:  {'position_in_set': 3, 'size_of_set': 3}"
"Label: Spike row gamma"
```

- The outer `ListState` surface carries `role: List` (`list/list.rs`'s `Render` impl
  sets `.role(Role::List)` unconditionally).
- **`List` wraps every row itself** in a `div` carrying `.role(Role::ListItem)`,
  `.aria_selected(..)`, `.aria_position_in_set(..)` and `.aria_size_of_set(..)`
  (`list/list.rs::render_list_item`). The delegate never has to ask for this.
- The row's own visible text is a separate `Label` node nested under that `ListItem`
  node, and reaches the tree **only** because the delegate's `render_item` wraps it
  in `text!` — exactly Task 6's rule. A bare `&str` child, or `ListItem`'s own
  `.accessibility_label(..)` builder (which sets `aria.label` on `ListItem`'s own
  `div` but never calls `.role(..)` on it — see "a latent trap" below), produces no
  node at all for the row's text.
- **Pattern for Phase 1:** `ListItem::new(id).child(text!(label))` as the delegate's
  `Self::Item`. Nothing else — selection/position/role come from `List` itself.

### A latent trap: `ListItem::accessibility_label` does nothing by itself

`ListItem::accessibility_label(label)` (`list/list_item.rs`) only calls
`.aria_label(label)` on the `ListItem`'s own `div`; it never calls `.role(..)` on it.
Per the rule above (id **and** role both required), that `div` never gets its own
a11y node, so the label is silently dropped — unless the row is *also* wrapped by
something that does set a role (as `List`'s own row wrapper does). A `ListItem` used
outside `List` (e.g. in a hand-rolled menu) with only `.accessibility_label(..)` and
no `.role(..)` of its own exposes nothing. Phase 1 should prefer a `text!` child
(works everywhere) over `.accessibility_label(..)` (only works if something else
supplies the role) for this widget.

## Input (`gpui_component::input::{Input, InputState}`)

Raw dump excerpt:

```
"TextInput: Spike input value | Spike input value | Spike input value"
```
(three repeated segments are the a11y value / an internal label-fallback / the probe's
own `|`-joined dump format — the single important fact is the value is present.)

- `Input::render` always calls `.role(accessibility_role)` (`input/input.rs`), so
  every input gets a node with a concrete role (`TextInput` by default; `EmailInput`,
  `UrlInput`, `PasswordInput`, etc. for the matching `InputContentType`;
  `MultilineTextInput` for a textarea).
- The current text becomes the node's accessible **value**
  (`accessibility_value`), computed from `state.text(cx)`, but **only while
  `window.is_a11y_active()` is true** (or under `#[cfg(feature = "test-support")]`).
  Before the tree is activated the value is not computed at all — this is correct
  laziness, not a bug, and matches how the automation `a11y` verb already activates
  the tree before reading it (`automation/mod.rs::a11y`).
- A non-mask placeholder becomes `aria_placeholder`; an explicit `.aria_label(..)` or,
  failing that, the placeholder, becomes the accessible name.
- **Pattern for Phase 1:** `InputState::new(window, cx).placeholder(text).default_value(text)`
  (or `.set_value(..)` later) and `Input::new(&state)`. No extra accessibility call
  needed for the common case.

## Switch (`gpui_component::switch::Switch`)

Raw dump excerpt:

```
"Switch: Spike switch {'toggled': True}"
```

- `Switch` wraps `gpui_base::Switch`, whose `render` sets `.role(Role::Switch)` and
  `.aria_toggled(..)` unconditionally (`gpui-base-0.7.0/src/switch.rs`).
- The accessible name defaults to the visible `.label(..)` text; `.accessibility_label(..)`
  overrides the *name* only, without changing what is drawn (useful when the visible
  label isn't a good name, e.g. an icon-only switch).
- **Pattern for Phase 1:** `Switch::new(id).label(text).checked(bool)`. No extra
  accessibility call needed.

## Dialog (`gpui_component::dialog::Dialog` via `Window::open_dialog`)

Raw dump excerpt (dialog open, title visible, no "Dialog:" line anywhere in the
tree):

```
"TextInput: Spike input value | Spike input value | Spike input value"
"Switch: Spike switch {'toggled': True}"
"Label: Spike dialog"
"Button: Close"
```

- The dialog **is** mounted: `window.open_dialog(cx, |dialog, _, _| dialog.title(text!("Spike dialog")))`
  opened on the spike view's first render works exactly as documented. It is mounted
  by `WindowState`, a `gpui_base::RootPlugin` gpui-component registers via
  `gpui_component::init` (called by `main.rs`'s `gpui_kit::init(cx)`), onto the
  window's `gpui_base::Root` — present because `gpui_kit::open_window`
  (what `shell/mod.rs` uses to open the panel) always wraps its content in that
  `Root`. **No workaround was needed to make the dialog appear or to make its title
  text reach the tree** — `text!` for the title, same as everywhere else, is enough.
- **The dialog's own container never gets a `role: Dialog` node**, confirmed both by
  reading the source and by the dump above (no `"Dialog: ..."` line exists anywhere,
  while the title `Label` and the `Close` `Button` — both nested *inside* that
  container — do appear). Root cause, read in
  `gpui-base-0.7.0/src/focus_trap.rs`:
  - `gpui_base::Dialog::render` (called by `gpui_component::dialog::Dialog::render`,
    which is what `open_dialog` ultimately builds) sets `.role(Role::Dialog)` on its
    "dialog-host" `div`, then chains `.focus_trap(format!("dialog-{layer}"), &self.focus)`.
  - `.focus_trap(..)` (`FocusTrapElement::focus_trap`) wraps that `div` in a new
    element, `FocusTrapContainer<Div>`, whose own `id()` returns the **trap's** id
    ("dialog-N"), not the inner div's ("dialog-host-N") — a different identity.
  - `FocusTrapContainer::prepaint` calls `self.base.prepaint(..)` **directly** —
    the inner `Div`'s raw `Element::prepaint`, not through gpui-pre's `Drawable`
    wrapper. The a11y-node-push logic (`self.element.a11y_role()` → push a node)
    lives only in `Drawable::prepaint` (`gpui-pre-0.3.7/src/element.rs`,
    `ElementDrawPhase::prepaint`), which `FocusTrapContainer` never calls for its
    wrapped element. So the inner div's `.role(Role::Dialog)` is set but never read.
  - `FocusTrapContainer` itself has no `a11y_role()` override either (defaults to
    `None`), so neither the trap nor the div it wraps produces a node.
  - The div's **children** (title, content, the close button) are unaffected: `Div`'s
    own `prepaint` recurses into each child through `Drawable` as usual, so they get
    their normal treatment. This is why the title `Label` and the `Close` `Button`
    still show up even though their direct parent (the dialog surface) is invisible
    to AccessKit.
  - Net effect: **any element that calls `.focus_trap(..)` loses its own a11y node**,
    full stop, regardless of what role it sets. This is not Dialog-specific; it would
    affect a sheet, a custom modal, or any hand-rolled focus-trapped container built
    the same way.
- `desktop-gpui/probes/a11y-spike.json` pins this as a known, *asserted* gap —
  `{"role": "Dialog"}` with `"absent": true` — rather than silently omitting a check,
  so a future gpui-pre/gpui-component upgrade that fixes `FocusTrapContainer` is
  noticed (that `see` step starts failing) instead of the gap persisting unnoticed.
- **Pattern for Phase 1:** use `Window::open_dialog` and `text!` for the title/body
  exactly as this spike does — the dialog opens and its text is legible to assistive
  technology. Do **not** write a proof that asserts the dialog surface itself carries
  `role: Dialog` (or any role) in this gpui-pre version; assert on the title/content
  text instead, which does reach the tree. If a future proof genuinely needs a
  role-bearing container around dialog-like content, it cannot use `.focus_trap(..)`
  (on this widget, or on any custom one) without losing that node — the smallest fix
  upstream would be for `FocusTrapContainer::prepaint` to route through the same
  `Drawable`-wrapping path normal children use, which this task did not attempt
  since it means editing vendored dependencies, out of this task's scope.

## Summary table

| Component | Role reaches tree | Visible text reaches tree | Extra call needed |
|---|---|---|---|
| List (row) | `ListItem` (from `List` itself) | yes, via `text!` child | no |
| List (container) | `List` | — | no |
| Input | `TextInput` (or content-type-specific) | yes, as the node's value, once a11y is active | no |
| Switch | `Switch` | yes, as the node's name (from `.label(..)`) | no |
| Dialog (container) | **no node at all** (`.focus_trap` bypass) | n/a | cannot fix from a caller; documented as a known gap |
| Dialog (title/content) | whatever the child is (`Label` for `text!`) | yes, via `text!` child | no |

## For Phase 1 (U4)

1. Wrap any visible text a proof will assert on in `text!`, never a bare `&str` —
   Task 6's rule, confirmed to hold the same way inside `List` rows and `Dialog`
   content.
2. `List`, `Input` and `Switch` need no extra accessibility call for the common case;
   just build them with their normal, documented API.
3. Do not assert a role on a `Dialog` (or any element reached through
   `.focus_trap(..)`) surface itself; assert on its title/content text, which is
   unaffected.
4. The automation `a11y` verb already activates the tree before reading it
   (`automation/mod.rs::a11y`), which is required for `Input`'s value to appear at
   all (`exposes_accessibility_value` is gated on `window.is_a11y_active()`) — a
   proof that reads `Input`'s value through some other path than `a11y` would need to
   activate accessibility itself first.

## Keyboard: typing and Tab focus (Task 11, proven live in a11y-spike.json)

- GPUI dispatches a key from the **focused** element up its dispatch path; with
  nothing focused it starts at the window's root node, which carries no key context,
  so neither typed text nor gpui-base `Root`'s `tab`/`shift-tab` bindings (context
  `Root`, `focus_next`/`focus_prev`) are reached and the automation `key` verb reports
  `handled: false`. A view must focus something when it opens (Phase 1: the browser's
  filter input, a settings/onboarding control).
- With an `Input` focused, the `key` verb's keystrokes (`Window::dispatch_keystroke`,
  which gives a printable key a simulated `key_char` and feeds it to the focused
  element's input handler) insert text: the `TextInput` node's value changes, and
  `backspace` deletes. No CGEvent, no Accessibility grant.
- Tab moves focus and the newly focused `Switch` (or `Button`: Tab inside the open
  dialog focuses its Close button) node carries the probe's `focused`
  flag (the dump's `gpui_focus`). The focused `Input` itself is **not** reported as
  `gpui_focus` in this gpui-pre version, so assert an Input's focus by typing into it,
  not by its `focused` flag.
- Escape closes a `Dialog` (gpui-component's own binding) and focus returns to what
  was focused before it opened.

