# Ubilink — agent notes

## UI work: native macOS 27, not the Graphical theme

Ubilink's look is **native macOS 27**: the system font (SF, `.SystemUIFont`) at macOS
text-style sizes, AppKit's system colours, the user's live system accent colour, and a
Liquid Glass panel (NSGlassEffectView on macOS 26+, vibrancy before). DECISIONS.md D26
records it, with every deviation (each one for WCAG AA).

- The single source of values is `design/ubilink-tokens.json`. It flows through
  `desktop-gpui/src/theme/convert.rs` into gpui-component `ThemeConfig`s, and
  `theme/accent.rs` derives the accent roles from the live accent. Views take colours
  from `cx.theme()` semantic roles and sizes from `theme::tokens`. No hex values in
  views; `px()` only where layout needs it.
- `GUI.md` and `gui/` (the Graphical "Ubilink" theme) are reference material. **Use
  their method** (inventory, map by semantic role, convert shared components before
  screens, then audit) through the skills below. **Do not use their colours, fonts,
  radii or spacing**: the native macOS 27 values above override them.
- Every text pair must reach 4.5:1 and non-text UI 3:1, in light and dark. Record any
  change made to reach that in D26's deviation table.
- The probes check the theme: `desktop-gpui/probes/theming.json`, `accent.json`, and
  the material assertion in `shell.json` (`desktop-gpui/probes/README.md`).

## Skills (`.agents/skills/`, see its README)

- `gpui-kit`, `gpui-kit-design-guides`: GPUI Kit v0.7.0 usage and design rules. The
  design guides apply to every GPUI change.
- `graphical-convert`, `graphical-audit`, `graphical-ui`: the conversion and audit
  method, applied with the native values above.
