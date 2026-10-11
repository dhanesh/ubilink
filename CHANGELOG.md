# Changelog

All notable changes to Ubilink are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/). The release pipeline publishes the
section for each version as that release's notes.

## [0.1.0] - unreleased

The first public release: a native macOS menu-bar app that copies a link to whatever
you're looking at and keeps a two-way graph of the links you make.

### Added
- **Capture** the resource in the app you're using with **Cmd+Shift+K**: Safari,
  Google Chrome, Arc, Brave, Microsoft Edge, Vivaldi, Opera, Finder, Bear and
  Things 3, plus any file.
- **Link graph** with backlinks: link two resources and each shows the other.
- **Link browser** (**Cmd+Shift+L**) to find, open and copy links.
- **Settings** (**Cmd+Shift+,**): permissions (Automation's status and a way back
  to permission setup), app adapters, and graph export / import / reset.
- **Custom app adapters** from your own `adapters.json` — trusted local config that
  runs with your permissions (see `docs/SUPPORTED-APPS.md`).
- **Permission onboarding** for macOS Automation, the one permission capture needs.
  Until Automation is granted, opening capture takes you to it; once granted, it
  stays out of the way. It also shows Accessibility's status, which no built-in
  app adapter needs.
- A universal (Apple silicon + Intel), notarized DMG.

### Changed
- Ubilink is now a native app built with GPUI: one process, no web view, a native
  blurred pop-up panel and a menu-bar icon. It keeps the name, bundle id
  (`org.dhanesh.ubilink`) and data folder of the earlier web-view build, so an
  existing link graph, adapter settings and `adapters.json` carry over unchanged.
- A native macOS 27 look: the system font, system colours in light and dark, buttons,
  switches and selection in your chosen accent colour (and following it when you change
  it), and a Liquid Glass panel on macOS 26 and later.

### Removed
- The Tauri web-view app and its web panel. The packaged app, the DMG and the release
  workflow now build the GPUI app; the release binary is checked to carry no test
  automation channel.

### Security
- One link-validation policy for capture, import and open: dangerous schemes
  (`javascript:`, `data:`, …) are always refused, `file:` links must be local, and
  links open without a shell (#3, #11).
- Links to apps, scripts and installers are revealed in Finder instead of run (#4).
- Imports are all-or-nothing and validated row by row (#3).
- Captures are bounded to 15 seconds and never freeze the rest of the app (#7).
- Graph database migrations are versioned and atomic (#8).
- Custom adapters are clearly trusted local config and can't make themselves
  built-in (#2).
- Groundwork for sandboxed third-party integrations (not enabled in the app yet):
  signatures cover the whole manifest (#1), a hardened Node sandbox with
  authenticated messages (#5), the focused resource only via the broker (#6), and a
  registry with publisher ownership, bounded requests, a manifest schema gate and
  durable storage (#9, #14).
- Development and release process: CI actions pinned to commit SHAs and a gated
  signing job (#10); agent edits to CI config, the Makefile and Claude settings are
  guarded, and refused in auto mode (#13, #15).

### Known limitations
- **macOS only** (10.15 or later). Windows and Linux capture is not supported yet.
- **Third-party integrations are not available in the app yet**; the sandbox,
  broker and registry are developer-facing and coming in a later release.
- **No automatic updates** — download new versions from GitHub Releases.
- If you ran a development build before, macOS asks for Automation permission
  again once.
