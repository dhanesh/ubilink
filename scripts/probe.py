#!/usr/bin/env python3
"""GPUI probe harness.

Phase 0 of the GPUI port (see .superpowers/sdd/2026-10-03-gpui-phase0-plan/)
needs a handful of small, scriptable checks that later tasks keep extending
(toolchain, then GPUI-specific build/runtime probes). This is the single
entry point for all of them, kept stdlib-only so it needs no setup beyond
python3 itself. Each check is its own subcommand; add new ones as the plan
adds tasks instead of growing this one into a monolith.

Usage:
    python3 scripts/probe.py toolchain --expect 1.98
    python3 scripts/probe.py deps-clean app/Cargo.toml
    python3 scripts/probe.py release-clean
    python3 scripts/probe.py run shell [--instances 2]
    python3 scripts/probe.py doctor [--json] [--build]

`run` is the instance runner and scenario engine (R8): see
desktop-gpui/probes/README.md for the scenario format.
"""

import argparse
import contextlib
import fcntl
import hashlib
import json
import os
import random
import re
import shutil
import signal
import socket
import sqlite3
import subprocess
import sys
import tempfile
import time
import unicodedata
from pathlib import Path

# scripts/probe.py -> repo root is this file's grandparent.
REPO_ROOT = Path(__file__).resolve().parent.parent

# Matches a GitHub Actions workflow `with: toolchain: "1.92.0"` input line
# (dtolnay/rust-toolchain's input), quoted or not. Deliberately anchored on
# the `toolchain:` key so it doesn't false-positive on prose mentioning
# "rust-toolchain.toml" in comments.
_TOOLCHAIN_INPUT_RE = re.compile(r'^\s*toolchain:\s*["\']?([0-9][\w.\-]*)["\']?\s*$')


def _iter_workflow_toolchain_lines(workflows_dir: Path):
    """Yield (file, lineno, version) for every `toolchain:` input line found."""
    for workflow_file in sorted(workflows_dir.glob("*.yml")):
        text = workflow_file.read_text(encoding="utf-8")
        for lineno, line in enumerate(text.splitlines(), start=1):
            m = _TOOLCHAIN_INPUT_RE.match(line)
            if m:
                yield workflow_file, lineno, m.group(1)


def cmd_toolchain(args: argparse.Namespace) -> int:
    """Check rustc --version matches --expect, and no workflow names a different one.

    R1 (task 1 of the GPUI plan) requires rust-toolchain.toml and every
    workflow's toolchain input to agree on one Rust version, since
    CI pins the same version the repo's `make verify` runs locally
    (see rust-toolchain.toml's comment on why they must match).
    """
    expect = args.expect
    failures = []

    # 1. rustc --version, run from the repo root so a rust-toolchain.toml
    #    override in this directory is honoured (rustup resolves per-cwd).
    try:
        proc = subprocess.run(
            ["rustc", "--version"],
            cwd=REPO_ROOT,
            capture_output=True,
            text=True,
            check=False,
        )
    except FileNotFoundError:
        print("FAIL: toolchain — rustc not found on PATH")
        return 1

    rustc_version = proc.stdout.strip()
    expected_prefix = f"rustc {expect}"
    if proc.returncode != 0 or not rustc_version.startswith(expected_prefix):
        failures.append(
            f"rustc --version printed {rustc_version!r}, expected it to start with {expected_prefix!r}"
        )

    # 2. Every workflow's `toolchain:` input must name a version starting
    #    with the same prefix — no straggler left on the old pin.
    workflows_dir = Path(args.workflows_dir) if args.workflows_dir else (REPO_ROOT / ".github" / "workflows")
    if not workflows_dir.is_dir():
        failures.append(f"workflows directory not found: {workflows_dir}")
    else:
        for workflow_file, lineno, version in _iter_workflow_toolchain_lines(workflows_dir):
            if not version.startswith(expect):
                rel = workflow_file
                try:
                    rel = workflow_file.relative_to(REPO_ROOT)
                except ValueError:
                    pass
                failures.append(f"{rel}:{lineno}: toolchain \"{version}\" does not start with \"{expect}\"")

    if failures:
        print(f"FAIL: toolchain — {len(failures)} issue(s):")
        for f in failures:
            print(f"  - {f}")
        return 1

    print(f"PASS: toolchain — rustc and all workflow inputs match {expect!r}")
    return 0


# Crate-name prefixes/exact names that must never appear in ubilink-app's (or any
# GUI-free crate's) dependency tree — the GUI toolkits task 3 (R3) says the GPUI
# port is a UI port, not a logic port, which only holds if the shared command
# layer stays physically free of BOTH GUI stacks, not just documented as such
# (same "physically true" pattern as ubilink-core's own no_network_deps guard).
_BANNED_DEP_PREFIXES = ("tauri", "gpui", "webkit")
_BANNED_DEP_EXACT = {"wry", "tao"}


def cmd_deps_clean(args: argparse.Namespace) -> int:
    """Fail if `manifest`'s dependency tree pulls in a Tauri or GPUI crate.

    R3 (task 3 of the GPUI Phase 0 plan) moves the Tauri command bodies into a
    GUI-free `ubilink-app` crate. `cargo tree` is the ground truth for what a
    crate actually depends on (not what a human remembers to keep out of
    Cargo.toml), so this shells out to it and scans every line's crate name.
    """
    manifest = Path(args.manifest)
    if not manifest.is_file():
        print(f"FAIL: deps-clean — manifest not found: {manifest}")
        return 1

    cmd = [
        "cargo",
        "tree",
        "--locked",
        "--manifest-path",
        str(manifest),
        "--prefix",
        "none",
        "-e",
        "normal",
    ]
    try:
        proc = subprocess.run(cmd, cwd=REPO_ROOT, capture_output=True, text=True, check=False)
    except FileNotFoundError:
        print("FAIL: deps-clean — cargo not found on PATH")
        return 1

    if proc.returncode != 0:
        print(f"FAIL: deps-clean — `cargo tree` exited {proc.returncode} for {manifest}")
        if proc.stderr.strip():
            print(proc.stderr.strip())
        return 1

    hits = []
    for line in proc.stdout.splitlines():
        name = line.strip().split()
        if not name:
            continue
        crate = name[0]
        lname = crate.lower()
        is_banned = lname in _BANNED_DEP_EXACT or any(
            lname.startswith(p) for p in _BANNED_DEP_PREFIXES
        )
        if is_banned and crate not in hits:
            hits.append(crate)

    if hits:
        print(
            f"FAIL: deps-clean — {manifest} depends on banned crate(s): {', '.join(sorted(hits))}"
        )
        return 1

    print(f"PASS: deps-clean — {manifest} has no tauri*/gpui*/wry/tao/webkit* crate in its tree")
    return 0


# Byte strings that only desktop-gpui's `automation` module puts in a binary (spec S1,
# TN1): its socket file name, a marker constant it prints at startup, and the verb
# names no other code in the tree uses. Generic verbs ("state", "key", "a11y",
# "opened", "metrics", "window_id") are left out on purpose — those words occur in
# GPUI and its dependencies too, so they would make the release check fail for
# reasons that have nothing to do with the automation channel.
_AUTOMATION_TOKENS = (
    "auto.sock",
    "UBILINK_AUTOMATION_MARKER",
    "open_view",
    "trigger_hotkey",
    "resign_key",
    "set_appearance",
    "load_fixture",
)

_GPUI_MANIFEST = REPO_ROOT / "desktop-gpui" / "Cargo.toml"
_GPUI_RELEASE_BIN = REPO_ROOT / "desktop-gpui" / "target" / "release" / "ubilink-next"


def _cargo_release_build(features):
    cmd = ["cargo", "build", "--locked", "--release", "--manifest-path", str(_GPUI_MANIFEST)]
    if features:
        cmd += ["--features", features]
    print(f"probe: {' '.join(cmd)}", flush=True)
    try:
        proc = subprocess.run(cmd, cwd=REPO_ROOT, check=False)
    except FileNotFoundError:
        return "cargo not found on PATH"
    if proc.returncode != 0:
        return f"`{' '.join(cmd)}` exited {proc.returncode}"
    if not _GPUI_RELEASE_BIN.is_file():
        return f"build succeeded but {_GPUI_RELEASE_BIN} is missing"
    return None


def _tokens_in_binary(path: Path):
    """Return the automation tokens `strings -a` finds in `path`, or an error string."""
    try:
        proc = subprocess.run(["strings", "-a", str(path)], capture_output=True, check=False)
    except FileNotFoundError:
        return "`strings` not found on PATH (install the Xcode command line tools)"
    if proc.returncode != 0:
        return f"`strings` exited {proc.returncode} on {path}"
    out = proc.stdout
    return [t for t in _AUTOMATION_TOKENS if t.encode() in out]


def cmd_release_clean(args: argparse.Namespace) -> int:
    """Prove the release binary has no automation surface, and that the check bites.

    S1/TN1 (GPUI Phase 0 R6): the automation channel is compiled only with the
    `automation` feature. A `#[cfg]` in the source is a claim; the shipped binary is
    the fact. So: build the release binary exactly as shipped (no features) and fail
    if `strings` finds the socket name, the marker or an automation-only verb name.
    Then build the same profile WITH the feature and require the marker and socket
    name to be found — otherwise a check that can never find anything would pass.
    The automation build overwrites target/release/ubilink-next, so the plain build is
    re-run last (a no-op relink from cache) to leave the shipped binary in place.
    """
    failures = []

    err = _cargo_release_build(None)
    if err:
        print(f"FAIL: release-clean — release build: {err}")
        return 1
    found = _tokens_in_binary(_GPUI_RELEASE_BIN)
    if isinstance(found, str):
        print(f"FAIL: release-clean — {found}")
        return 1
    if found:
        failures.append(f"release binary contains automation token(s): {', '.join(found)}")
    else:
        print(f"probe: release binary clean of {len(_AUTOMATION_TOKENS)} automation tokens")

    if not args.skip_bite_check:
        err = _cargo_release_build("automation")
        if err:
            print(f"FAIL: release-clean — automation build: {err}")
            return 1
        found_auto = _tokens_in_binary(_GPUI_RELEASE_BIN)
        if isinstance(found_auto, str):
            print(f"FAIL: release-clean — {found_auto}")
            return 1
        # Every token must be visible in the automation build, or its absence from the
        # release build proves nothing (desktop-gpui logs its verb list at startup
        # for exactly this reason: LLVM turns short string matches into integer
        # compares, which leave no string behind).
        missing = [t for t in _AUTOMATION_TOKENS if t not in found_auto]
        if missing:
            failures.append(
                "the check does not bite: an --features automation build lacks "
                + ", ".join(missing)
            )
        else:
            print(
                "probe: automation build contains "
                f"{len(found_auto)}/{len(_AUTOMATION_TOKENS)} tokens ({', '.join(found_auto)})"
            )
        # Leave the shipped (feature-less) binary in target/release.
        err = _cargo_release_build(None)
        if err:
            failures.append(f"rebuilding the release binary afterwards: {err}")
        else:
            again = _tokens_in_binary(_GPUI_RELEASE_BIN)
            if again:
                failures.append(
                    f"target/release/ubilink-next after the rebuild still carries: {again}"
                )

    if failures:
        print(f"FAIL: release-clean — {len(failures)} issue(s):")
        for f in failures:
            print(f"  - {f}")
        return 1
    print("PASS: release-clean — release binary has no automation socket, marker or verb names")
    return 0


# ══════════════════════════════════════════════════════════════════════════
# `fixtures`: Task 10 (Phase 0 R10) — the data Phase 1's probes load
# ══════════════════════════════════════════════════════════════════════════
#
# desktop-gpui/probes/fixtures/*.json is committed test data, not code: capture
# results, permission states, link-graph JSON in ubilink-core's own export/import
# schema (src-tauri/src/graph.rs's `export_json`/`import_json`, schema
# "ubilink.graph.v3") and a settings adapter list. Phase 1's views (R1, R3, R4,
# R5, R9, R10, R11) build their consuming code against
# desktop-gpui/probes/fixtures/README.md's documented shapes, so `--check`
# validates every fixture against that doc — a typo or a hand edit that drifts
# from the schema is caught here, not discovered mid-Phase-1. `--generate`
# reproduces the one generated fixture (the 5,000-resource graph) byte-for-byte
# from its seed, so the committed file can never silently diverge from what the
# generator would produce.

FIXTURES_DIR = REPO_ROOT / "desktop-gpui" / "probes" / "fixtures"

# The 5,000-resource graph fixture's seed (Phase 1 R4/R5): fixed so `--generate`
# reproduces the committed file exactly on every machine. Changing it is a
# deliberate fixture update (re-run `--generate` and commit the result), not
# routine maintenance.
GRAPH_5000_SEED = 20261003
GRAPH_5000_COUNT = 5000
GRAPH_5000_BASE_MS = 1_700_000_000_000


def generate_graph_5000(seed: int = GRAPH_5000_SEED, count: int = GRAPH_5000_COUNT) -> dict:
    """The 5,000-resource graph fixture (R4: a virtualised list at 5,000 "links";
    R5: the fuzzy filter re-filtering 5,000 of them), reproducible from `seed` alone.

    Resources are named/URI'd in plain index order (`Resource 00001`, ...) so the
    file reads the same every time, but CAPTURE order (`created_at`) is a seeded
    shuffle of that index order — the same way a real graph's alphabetical order
    and its recency order disagree. No links: R4/R5 exercise the resource list and
    the filter, not graph edges (graph-backlinks*.json covers the edge case). Pure
    and deterministic, so `--check` can tell a hand-edited file from a freshly
    generated one by exact structural equality.
    """
    rng = random.Random(seed)
    order = list(range(1, count + 1))
    rng.shuffle(order)
    capture_order = {idx: pos for pos, idx in enumerate(order)}
    resources = [
        {
            "guid": f"g{idx:05d}",
            "name": f"Resource {idx:05d}",
            "uri": f"https://fixture-{idx:05d}.example.test/resource",
            "created_at": GRAPH_5000_BASE_MS + capture_order[idx] * 1000,
        }
        for idx in range(1, count + 1)
    ]
    return {"schema": "ubilink.graph.v3", "resources": resources, "links": []}


def is_subsequence(query: str, text: str) -> bool:
    """Whether every character of `query` appears in `text`, in order (not
    necessarily contiguous), case-insensitively — the match rule R5's fuzzy filter
    must rank above any other kind of match. An empty query never matches (the same
    "no vacuous checks" rule the rest of the harness follows): ranking-golden.json's
    queries and decoys both lean on that to mean something.
    """
    if not query:
        return False
    q = query.lower()
    i = 0
    for ch in text.lower():
        if i < len(q) and ch == q[i]:
            i += 1
            if i == len(q):
                return True
    return False


def min_window_span(query: str, text: str):
    """(length, start) of the SHORTEST contiguous window of `text` that embeds
    `query` as an in-order subsequence, preferring the earliest `start` among
    windows tied at that minimum length; `None` if `query` is not a subsequence
    of `text` at all. Case-sensitive and otherwise pure — `rank` below does the
    case-folding, so this is independently testable against exact strings.

    This is the "minimum window subsequence" problem (LeetCode 727), not the
    greedy-leftmost embedding: `is_subsequence`'s own left-to-right scan finds
    *a* match but not necessarily the tightest one, which matters once two
    corpus entries both match and the fixtures/README.md ranking rule orders
    them by how tight the match is. Classic two-pass fix: scan `text` forward,
    greedily completing one embedding of `query`; every time an embedding
    completes at some end index, walk backward from that end to find the
    LATEST possible start for a window ending there (equivalently, the
    shortest window ending at that position); record it, then resume the
    forward scan from just after that start — never from the original end —
    so a still-shorter window starting later is never missed. Because the
    scan only moves forward, each window found this way starts strictly after
    the previous one, so the first minimum-length window found is, by
    construction, also the one with the earliest start among every window
    tied at that length (ties are handled anyway, defensively).
    """
    n, m = len(text), len(query)
    if m == 0:
        return None
    best_len = None
    best_start = None
    i = 0
    while i < n:
        j, k = 0, i
        while k < n:
            if text[k] == query[j]:
                j += 1
                if j == m:
                    break
            k += 1
        if j != m:
            break  # no more complete embeddings starting at or after i
        end = k
        j2, start = m - 1, end
        while j2 >= 0:
            if text[start] == query[j2]:
                j2 -= 1
            start -= 1
        start += 1
        length = end - start + 1
        if best_len is None or length < best_len or (length == best_len and start < best_start):
            best_len, best_start = length, start
        i = start + 1
    return (best_len, best_start) if best_len is not None else None


def rank(query: str, uris) -> list:
    """The reference ranking rule R5's filter must reproduce (normative spec in
    fixtures/README.md, with pseudocode and worked examples). Matches against
    each uri in `uris` (case-folded; names are not matched — see the README for
    why), ordered by:

      1. the shortest minimum-window span containing the match (`min_window_span`,
         NOT the greedy-leftmost embedding `is_subsequence` would find)
      2. that window's start index (earliest first)
      3. no word-boundary/prefix bonus — deliberately: this is a single,
         unambiguous number with no second scoring pass bolted on
      4. the uri itself, case-folded, ascending — a total, stable tie-break
         that does not depend on `uris`' own order (so a corpus handed in any
         order produces the same ranking)

    Non-matches are dropped entirely (never ranked, however "close"). Returns
    the matching uris in ranked order (every match, not just a top-N slice —
    callers take `[:3]` for a "top 3").
    """
    q = query.lower()
    hits = []
    for uri in uris:
        span = min_window_span(q, uri.lower())
        if span is None:
            continue
        length, start = span
        hits.append((length, start, uri.lower(), uri))
    hits.sort(key=lambda h: (h[0], h[1], h[2]))
    return [h[3] for h in hits]


def _load_fixture_json(path: Path):
    if not path.is_file():
        raise ValueError(f"missing file: {path}")
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as e:
        raise ValueError(f"bad JSON: {e}")


def _validate_graph_shape(doc) -> list:
    """Generic ubilink.graph.v3 checks shared by every graph-*.json fixture: the
    schema tag, that every link's endpoints resolve to a declared resource guid
    (what `LinkGraph::import_json` requires to write the link at all), no self-links,
    and that resource guids/uris are each unique. Specific fixtures layer their own
    checks (exact counts, documented order, ...) on top of this.
    """
    issues = []
    if doc.get("schema") != "ubilink.graph.v3":
        issues.append(f"schema must be 'ubilink.graph.v3', got {doc.get('schema')!r}")
        return issues
    resources, links = doc.get("resources"), doc.get("links")
    if not isinstance(resources, list):
        issues.append("resources must be a list")
        resources = []
    if not isinstance(links, list):
        issues.append("links must be a list")
        links = []
    guids, uris = set(), set()
    for i, r in enumerate(resources):
        if not isinstance(r, dict):
            issues.append(f"resources[{i}] must be an object")
            continue
        for k in ("guid", "name", "uri"):
            if not isinstance(r.get(k), str) or not r[k]:
                issues.append(f"resources[{i}].{k} must be a non-empty string")
        g, u = r.get("guid"), r.get("uri")
        if isinstance(g, str):
            if g in guids:
                issues.append(f"resources[{i}].guid {g!r} is not unique")
            guids.add(g)
        if isinstance(u, str):
            if u in uris:
                issues.append(f"resources[{i}].uri {u!r} is not unique")
            uris.add(u)
        if r.get("created_at") is not None and not isinstance(r.get("created_at"), int):
            issues.append(f"resources[{i}].created_at must be an integer or null")
    for i, l in enumerate(links):
        if not isinstance(l, dict):
            issues.append(f"links[{i}] must be an object")
            continue
        a, b = l.get("a"), l.get("b")
        if a not in guids:
            issues.append(f"links[{i}].a {a!r} is not a declared resource guid")
        if b not in guids:
            issues.append(f"links[{i}].b {b!r} is not a declared resource guid")
        if a == b:
            issues.append(f"links[{i}] is a self-link ({a!r})")
        if l.get("created_at") is not None and not isinstance(l.get("created_at"), int):
            issues.append(f"links[{i}].created_at must be an integer or null")
    return issues


def _validate_graph_empty(doc) -> list:
    issues = _validate_graph_shape(doc)
    if doc.get("resources") != []:
        issues.append("must have no resources")
    if doc.get("links") != []:
        issues.append("must have no links")
    return issues


def _validate_graph_5000(doc) -> list:
    issues = _validate_graph_shape(doc)
    resources = doc.get("resources") if isinstance(doc.get("resources"), list) else []
    if len(resources) != GRAPH_5000_COUNT:
        issues.append(f"must have exactly {GRAPH_5000_COUNT} resources, got {len(resources)}")
    uris = {r["uri"] for r in resources if isinstance(r, dict) and isinstance(r.get("uri"), str)}
    if len(uris) != len(resources):
        issues.append("every resource's uri must be unique")
    if doc.get("links"):
        issues.append("carries no links (R4/R5 exercise the resource list, not edges)")
    if doc != generate_graph_5000():
        issues.append(
            f"differs from `probe.py fixtures --generate` (seed {GRAPH_5000_SEED}); "
            "run --generate and commit the result"
        )
    return issues


# Backlinks fixtures (R3): the subject's newest-first backlink order, by uri, and
# the decoy that must never appear. Hardcoded (not derived from the file) so a hand
# edit that reorders the JSON or nudges a timestamp is caught instead of the check
# silently re-deriving a new "documented" order from whatever is there.
_BACKLINKS_SUBJECT_URI = "ubilink-fixture://subject"
_BACKLINKS_DECOY_URI = "https://decoy.example/unrelated"
_BACKLINKS_ORDER_NEWEST_FIRST = [
    "https://gamma.example/three",
    "https://beta.example/two",
    "https://alpha.example/one",
]
_BACKLINKS_NEWER_ORDER_NEWEST_FIRST = [
    "https://delta.example/four",
    "https://gamma.example/three",
    "https://beta.example/two",
    "https://alpha.example/one",
]


def _validate_backlinks_doc(doc, expected_order: list) -> list:
    issues = _validate_graph_shape(doc)
    if issues:
        return issues
    resources = doc["resources"]
    uri_of = {r["guid"]: r["uri"] for r in resources}
    subj = next((r for r in resources if r["uri"] == _BACKLINKS_SUBJECT_URI), None)
    if subj is None:
        return issues + [f"missing the subject resource {_BACKLINKS_SUBJECT_URI!r}"]
    if not any(r["uri"] == _BACKLINKS_DECOY_URI for r in resources):
        issues.append(f"missing the decoy resource {_BACKLINKS_DECOY_URI!r}")
    touching = []
    for l in doc["links"]:
        if subj["guid"] not in (l["a"], l["b"]):
            continue
        other = l["b"] if l["a"] == subj["guid"] else l["a"]
        touching.append((uri_of.get(other), l.get("created_at")))
    if any(u == _BACKLINKS_DECOY_URI for u, _ in touching):
        issues.append("the decoy must not be linked to the subject")
    if any(t is None for _, t in touching):
        issues.append("every backlink's link must have an explicit created_at")
        return issues
    times = [t for _, t in touching]
    if len(set(times)) != len(times):
        issues.append("backlink timestamps must be distinct")
    got_order = [u for u, _ in sorted(touching, key=lambda t: t[1], reverse=True)]
    if got_order != expected_order:
        issues.append(f"newest-first order must be {expected_order}, got {got_order}")
    return issues


def _validate_capture_ok(doc) -> list:
    issues = []
    if "error" in doc:
        issues.append("must not carry an error (that is capture-failed.json's job)")
    if "delay_ms" in doc:
        issues.append("must not carry delay_ms (that is capture-delayed.json's job)")
    for k in ("name", "uri", "app"):
        if not isinstance(doc.get(k), str) or not doc[k].strip():
            issues.append(f"{k} must be a non-empty string")
    return issues


def _validate_capture_delayed(doc) -> list:
    issues = _validate_capture_ok({k: v for k, v in doc.items() if k != "delay_ms"})
    delay = doc.get("delay_ms")
    if not isinstance(delay, int) or isinstance(delay, bool):
        issues.append("delay_ms must be an integer")
    elif delay != 300:
        issues.append(f"delay_ms must be 300 (R1's proof fixture), got {delay}")
    return issues


def _validate_capture_failed(doc) -> list:
    issues = []
    for k in ("name", "uri", "app", "delay_ms"):
        if k in doc:
            issues.append(f"must not carry {k} (this is the failure shape)")
    err = doc.get("error")
    if not isinstance(err, str) or not err.strip():
        issues.append("error must be a non-empty string")
    elif len(err.split()) < 4:
        issues.append("error must explain what failed and what to do (U3), not a terse code")
    return issues


_PERMISSION_STATES = {"granted", "denied", "not_determined"}


def _validate_permission_doc(doc) -> list:
    issues = []
    if set(doc) != {"accessibility", "automation"}:
        issues.append(f"must have exactly accessibility+automation, got {sorted(doc)}")
    if not isinstance(doc.get("accessibility"), bool):
        issues.append("accessibility must be a boolean")
    if doc.get("automation") not in _PERMISSION_STATES:
        issues.append(f"automation must be one of {sorted(_PERMISSION_STATES)}")
    return issues


def _validate_permissions_two_missing(doc) -> list:
    issues = _validate_permission_doc(doc)
    if not issues and (doc["accessibility"] or doc["automation"] == "granted"):
        issues.append("both permissions must be missing (onboarding shows two steps)")
    return issues


def _validate_permissions_all_granted(doc) -> list:
    issues = _validate_permission_doc(doc)
    if not issues and not (doc["accessibility"] and doc["automation"] == "granted"):
        issues.append("both permissions must be granted (the flipped Re-check state)")
    return issues


_ADAPTER_DEFAULTS_JSON = REPO_ROOT / "src-tauri" / "src" / "adapters.default.json"


def _validate_adapters(doc) -> list:
    issues = []
    seen = set()
    for i, row in enumerate(doc):
        where = f"[{i}]"
        if not isinstance(row, dict) or set(row) != {"app", "builtin", "user_defined", "enabled", "scheme"}:
            issues.append(f"{where}: must be {{app, builtin, user_defined, enabled, scheme}}")
            continue
        app = row["app"]
        if not isinstance(app, str) or not app:
            issues.append(f"{where}: app must be a non-empty string")
        elif app.lower() in seen:
            issues.append(f"{where}: duplicate app {app!r}")
        seen.add(str(app).lower())
        for k in ("builtin", "user_defined", "enabled"):
            if not isinstance(row.get(k), bool):
                issues.append(f"{where} ({app!r}): {k} must be a boolean")
        if row.get("scheme") is not None and not isinstance(row.get("scheme"), str):
            issues.append(f"{where} ({app!r}): scheme must be a string or null")
    names = [row.get("app") for row in doc if isinstance(row, dict)]
    if names != sorted(names, key=lambda n: str(n).lower()):
        issues.append("apps must be sorted case-insensitively (matches list_plugins' order)")
    # Cross-check against the real embedded defaults (app/src/plugins.rs's
    # list_plugins reads ubilink_core::AdapterRegistry::defaults(), which embeds
    # this file), so the fixture can't silently drift from what settings-adapters
    # actually sees.
    try:
        defaults = json.loads(_ADAPTER_DEFAULTS_JSON.read_text(encoding="utf-8"))
    except (OSError, ValueError) as e:
        issues.append(f"could not read {_ADAPTER_DEFAULTS_JSON} to cross-check: {e}")
        return issues
    want = sorted(
        ((d.get("app"), bool(d.get("builtin", False))) for d in defaults),
        key=lambda t: str(t[0]).lower(),
    )
    got = sorted(
        ((row.get("app"), row.get("builtin")) for row in doc if isinstance(row, dict)),
        key=lambda t: str(t[0]).lower(),
    )
    if got != want:
        issues.append(f"must match {_ADAPTER_DEFAULTS_JSON}'s app+builtin set exactly: want {want}, got {got}")
    return issues


def _validate_ranking_golden(doc, corpus_uris: list) -> list:
    """Validate ranking-golden.json against `rank()`, the normative scorer
    (fixtures/README.md): every `expect_top3` must equal `rank(query, corpus)`'s
    own top 3 EXACTLY (not just "each is some kind of match" — order included),
    so the table can never silently drift from the one rule it claims to follow.
    `decoys` are checked separately: each must be in the corpus and must not
    appear in `rank()`'s result at all (not a match by any margin).
    """
    corpus_set = set(corpus_uris)
    issues = []
    if len(doc) != 20:
        issues.append(f"must have exactly 20 queries (the golden table), got {len(doc)}")
    seen = set()
    for i, entry in enumerate(doc):
        where = f"[{i}]"
        if not isinstance(entry, dict) or set(entry) != {"query", "expect_top3", "decoys"}:
            issues.append(f"{where}: must be {{query, expect_top3, decoys}}")
            continue
        q = entry["query"]
        if not isinstance(q, str) or not q:
            issues.append(f"{where}: query must be a non-empty string")
            continue
        if q in seen:
            issues.append(f"{where} ({q!r}): duplicate query")
        seen.add(q)
        top3 = entry["expect_top3"] if isinstance(entry.get("expect_top3"), list) else None
        if not top3 or not (1 <= len(top3) <= 3):
            issues.append(f"{where} ({q!r}): expect_top3 must be a list of 1-3 uris")
            top3 = None
        decoys = entry["decoys"] if isinstance(entry.get("decoys"), list) else None
        if not decoys:
            issues.append(f"{where} ({q!r}): decoys must be a non-empty list")
            decoys = []
        if top3 is not None:
            for uri in top3:
                if uri not in corpus_set:
                    issues.append(f"{where} ({q!r}): expected uri {uri!r} is not in graph-ranking.json")
            computed = rank(q, corpus_uris)[:3]
            if top3 != computed:
                issues.append(
                    f"{where} ({q!r}): expect_top3 {top3} disagrees with the reference "
                    f"scorer `rank(query, corpus)`, which ranks {computed} first"
                )
        for uri in decoys:
            if uri not in corpus_set:
                issues.append(f"{where} ({q!r}): decoy {uri!r} is not in graph-ranking.json")
            elif uri in rank(q, corpus_uris):
                issues.append(f"{where} ({q!r}): decoy {uri!r} IS ranked (matches), not a valid decoy")
    return issues


# Fixtures whose top level is a JSON list rather than an object.
_LIST_FIXTURES = {"adapters.json", "ranking-golden.json"}

_FIXTURE_VALIDATORS = {
    "capture-ok.json": _validate_capture_ok,
    "capture-delayed.json": _validate_capture_delayed,
    "capture-failed.json": _validate_capture_failed,
    "graph-backlinks.json": lambda d: _validate_backlinks_doc(d, _BACKLINKS_ORDER_NEWEST_FIRST),
    "graph-backlinks-newer.json": lambda d: _validate_backlinks_doc(d, _BACKLINKS_NEWER_ORDER_NEWEST_FIRST),
    "graph-5000.json": _validate_graph_5000,
    "graph-ranking.json": _validate_graph_shape,
    "permissions-two-missing.json": _validate_permissions_two_missing,
    "permissions-all-granted.json": _validate_permissions_all_granted,
    "adapters.json": _validate_adapters,
    "graph-empty.json": _validate_graph_empty,
}

# Every fixture `--check` walks, in the order desktop-gpui/probes/fixtures/README.md
# documents them. ranking-golden.json is cross-checked against graph-ranking.json
# below rather than through `_FIXTURE_VALIDATORS`, since it needs that file's corpus.
FIXTURE_FILES = [
    "capture-ok.json",
    "capture-delayed.json",
    "capture-failed.json",
    "graph-backlinks.json",
    "graph-backlinks-newer.json",
    "graph-5000.json",
    "graph-ranking.json",
    "ranking-golden.json",
    "permissions-two-missing.json",
    "permissions-all-granted.json",
    "adapters.json",
    "graph-empty.json",
]


def _validate_fixture_file(name: str, path: Path) -> list:
    doc = _load_fixture_json(path)
    expect_list = name in _LIST_FIXTURES
    if expect_list != isinstance(doc, list):
        return [f"must be a JSON {'list' if expect_list else 'object'}"]
    if name == "ranking-golden.json":
        try:
            corpus = _load_fixture_json(FIXTURES_DIR / "graph-ranking.json")
        except ValueError as e:
            return [f"could not read graph-ranking.json to cross-check: {e}"]
        corpus_issues = _validate_graph_shape(corpus)
        corpus_uris = [
            r["uri"] for r in corpus.get("resources", []) if isinstance(r, dict) and isinstance(r.get("uri"), str)
        ]
        return _validate_ranking_golden(doc, corpus_uris) + [f"graph-ranking.json: {i}" for i in corpus_issues]
    validator = _FIXTURE_VALIDATORS.get(name)
    if validator is None:
        return [f"no validator registered for {name}"]
    return validator(doc)


def _fixtures_check() -> int:
    ok = True
    for name in FIXTURE_FILES:
        try:
            issues = _validate_fixture_file(name, FIXTURES_DIR / name)
        except ValueError as e:
            issues = [str(e)]
        if issues:
            ok = False
            print(f"FAIL: fixtures — {name}: {issues[0]}")
            for extra in issues[1:]:
                print(f"  - {extra}")
        else:
            print(f"PASS: fixtures — {name}")
    return 0 if ok else 1


def _fixtures_generate() -> int:
    doc = generate_graph_5000()
    path = FIXTURES_DIR / "graph-5000.json"
    path.write_text(json.dumps(doc, separators=(",", ":")) + "\n", encoding="utf-8")
    print(f"probe: wrote {path} ({len(doc['resources'])} resources, seed={GRAPH_5000_SEED})")
    return 0


def cmd_fixtures(args: argparse.Namespace) -> int:
    """`--check` validates every desktop-gpui/probes/fixtures/*.json against its
    documented schema; `--generate` regenerates graph-5000.json from its seed (R10).
    """
    if args.generate:
        return _fixtures_generate()
    return _fixtures_check()


# ══════════════════════════════════════════════════════════════════════════
# `run`: the instance runner and scenario engine (R8, O1, O4, TN5)
# ══════════════════════════════════════════════════════════════════════════
#
# R8: launch isolated automation instances of ubilink-next (own data dir, sockets,
# fixtures, no real hotkeys), drive them through the automation socket (R6), and
# check what they show with scenario files under desktop-gpui/probes/. The scenario
# format is documented in desktop-gpui/probes/README.md; keep the two in step.

# Everything the harness keeps between runs: the warm cargo target dir, compiled
# Swift helpers and the desktop lock. ~/Library/Caches because macOS may purge it
# and nothing in it is precious (a purge only costs a rebuild). The env override
# exists for the self-tests, which must not touch the real cache.
CACHE_ROOT = Path(
    os.environ.get("UBILINK_PROBE_CACHE") or (Path.home() / "Library" / "Caches" / "ubilink-probe")
)
PROBES_DIR = REPO_ROOT / "desktop-gpui" / "probes"
_GPUI_SOURCE_TARGET = REPO_ROOT / "desktop-gpui" / "target"
_OCR_SWIFT = REPO_ROOT / "scripts" / "ocr.swift"
_QUARTZ_SWIFT = REPO_ROOT / "scripts" / "probe-helpers" / "quartz.swift"
_ACTIVATOR_SWIFT = REPO_ROOT / "scripts" / "probe-helpers" / "activator.swift"

# How long an instance may take to serve its socket after launch. A debug GPUI
# binary starts in about a second; the margin covers a cold Metal shader cache.
_LAUNCH_TIMEOUT_S = 60.0
# The app answers a request within its own 30 s REPLY_TIMEOUT (automation/mod.rs);
# wait a little longer so its error, not ours, reaches the report.
_CALL_TIMEOUT_S = 40.0
# How long `run` waits for another probe to release the desktop.
_LOCK_TIMEOUT_S = 900.0


class ScenarioError(Exception):
    """The scenario file itself is wrong (bad JSON, unknown step, bad option)."""


class EnvBlocked(Exception):
    """The host cannot run this step (missing TCC grant, too few displays): not a product FAIL."""


class StepFailed(Exception):
    """The product did not do what the step expects."""

    def __init__(self, message, expected=None, actual=None):
        super().__init__(message)
        self.expected = expected
        self.actual = actual


class ProbeError(Exception):
    """The harness could not do its job (build failed, instance never served its socket)."""


class CallError(Exception):
    """The app answered a request with {"ok": false}."""


# ── warm build (R8: "seed each build from a warm target directory") ─────────


def _seed_target_cache(cache_target: Path, source_target: Path) -> str:
    """Seed `cache_target` from `source_target` with an APFS clone; return what happened.

    The factory verifier runs each command in a fresh git clone, whose own target dir
    is empty: a cold GPUI build there takes many minutes. A shared CARGO_TARGET_DIR
    under the cache keeps every registry crate built across clones (only the path
    crates rebuild, since their source path differs). `cp -c` clones on APFS, so
    seeding from the repo's existing target costs seconds and no disk. The clone goes
    to a temp name and is renamed into place, so a concurrent probe never builds into
    a half-copied directory.
    """
    if cache_target.is_dir() and any(cache_target.iterdir()):
        return "warm"
    if not source_target.is_dir():
        return f"cold (nothing to seed from: {source_target} is missing)"
    cache_target.parent.mkdir(parents=True, exist_ok=True)
    staging = cache_target.with_name(f"{cache_target.name}.seed-{os.getpid()}")
    proc = subprocess.run(
        ["cp", "-c", "-R", str(source_target), str(staging)], capture_output=True, text=True
    )
    if proc.returncode != 0:
        shutil.rmtree(staging, ignore_errors=True)
        # Not falling back to a full copy: that is gigabytes, and a cold build is
        # merely slow, never wrong.
        return f"cold (APFS clone failed: {proc.stderr.strip() or proc.returncode})"
    try:
        if cache_target.is_dir():
            cache_target.rmdir()  # empty, checked above
        os.rename(staging, cache_target)
    except OSError:
        # Another probe seeded it first; theirs is as good as ours.
        shutil.rmtree(staging, ignore_errors=True)
        return "warm (seeded concurrently by another probe)"
    return f"seeded by APFS clone of {source_target}"


class Build:
    """A binary this run owns, and what it was built from (evidence names what ran)."""

    def __init__(self, path: Path, profile: str, sha256: str, commit: str, dirty: bool):
        self.path = path
        self.profile = profile
        self.sha256 = sha256
        self.commit = commit
        self.dirty = dirty

    def describe(self) -> str:
        return (
            f"{self.profile} binary sha256={self.sha256} "
            f"commit={self.commit}{'+dirty' if self.dirty else ''} ({self.path})"
        )


def _sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


# The build inputs whose uncommitted edits make a binary differ from its commit.
_BUILD_INPUTS = ("desktop-gpui", "app", "src-tauri", "design")


def source_revision(repo: Path = None) -> tuple:
    """(HEAD sha, dirty) of the repo this probe runs in.

    Dirty means a tracked or untracked change under the build inputs: the binary is
    then not exactly the commit, and the evidence says so. Untracked files elsewhere
    (scratch dirs, .skill-contract/) cannot reach the binary and do not count.
    """
    repo = repo or REPO_ROOT
    head = subprocess.run(
        ["git", "rev-parse", "HEAD"], cwd=repo, capture_output=True, text=True
    )
    if head.returncode != 0:
        return "unknown", True
    status = subprocess.run(
        ["git", "status", "--porcelain", "--", *_BUILD_INPUTS],
        cwd=repo, capture_output=True, text=True,
    )
    return head.stdout.strip(), status.returncode != 0 or bool(status.stdout.strip())


def executable_from_cargo_json(stdout: str, bin_name: str = "ubilink-next"):
    """The `executable` of the compiler-artifact message for `bin_name`, or None.

    Taken from cargo's own report instead of assumed (`target/debug/<name>`): the
    profile, target dir or a future target triple can all move it.
    """
    exe = None
    for line in stdout.splitlines():
        line = line.strip()
        if not line.startswith("{"):
            continue
        try:
            msg = json.loads(line)
        except ValueError:
            continue
        tgt = msg.get("target") or {}
        if (
            msg.get("reason") == "compiler-artifact"
            and tgt.get("name") == bin_name
            and "bin" in (tgt.get("kind") or [])
            and msg.get("executable")
        ):
            exe = msg["executable"]
    return exe


_builds = {}


def build_binary(run_dir: Path, release: bool = False, out=print) -> Build:
    """Build ubilink-next and take this run's own copy of it, under the target lock.

    The warm target dir is shared by every clone and commit (that is its point), so
    its `ubilink-next` is whatever the last probe built. Building and cloning the
    produced executable into `run_dir` both happen while holding
    CACHE_ROOT/target.lock: no other probe can replace the binary between our build
    finishing and our copy, so the instances run exactly this commit's code even when
    parallel Phase 1 tasks probe different commits. The build runs before (and
    outside) the desktop lock, so a long build never blocks another probe's GUI work.

    `release=False` is the `--features automation` debug build every scenario drives;
    `release=True` is the stripped release build as shipped (no automation channel),
    for probes that measure the real app (Phase 1 R15).
    """
    key = "release" if release else "automation"
    if key in _builds:
        return _builds[key]
    target = CACHE_ROOT / "target"
    cmd = ["cargo", "build", "--locked", "--message-format=json-render-diagnostics"]
    cmd += ["--release"] if release else ["--features", "automation"]
    cmd += ["--manifest-path", str(_GPUI_MANIFEST)]
    env = dict(os.environ, CARGO_TARGET_DIR=str(target))
    commit, dirty = source_revision()
    with file_lock(CACHE_ROOT / "target.lock", "target lock", out=out):
        note = _seed_target_cache(target, _GPUI_SOURCE_TARGET)
        out(f"probe: target dir {target}: {note}")
        out(f"probe: CARGO_TARGET_DIR={target} {' '.join(cmd)}")
        try:
            # JSON messages on stdout (parsed below); cargo's progress and rendered
            # diagnostics on our stderr, so stdout stays the step report.
            proc = subprocess.run(
                cmd, cwd=REPO_ROOT, env=env, stdout=subprocess.PIPE, text=True, check=False
            )
        except FileNotFoundError:
            raise ProbeError("cargo not found on PATH")
        if proc.returncode != 0:
            raise ProbeError(f"{key} build exited {proc.returncode}")
        exe = executable_from_cargo_json(proc.stdout)
        if not exe or not Path(exe).is_file():
            raise ProbeError(f"cargo reported no ubilink-next executable for the {key} build")
        own = Path(run_dir) / f"ubilink-next-{key}"
        if subprocess.run(["cp", "-c", exe, str(own)], capture_output=True).returncode != 0:
            shutil.copy2(exe, own)
    build = Build(own, key, _sha256_file(own), commit, dirty)
    out(f"probe: {build.describe()}")
    _builds[key] = build
    return build


# ── Swift helpers (OCR, Quartz window list, cursor, pasteboard) ─────────────


def _helper(source: Path) -> Path:
    """Compile `source` with swiftc -O into the cache (once per source revision).

    The binary name carries the source's hash, so an edited helper is rebuilt instead
    of a stale one being run, and two probe clones on different revisions never
    overwrite each other's helper.
    """
    digest = hashlib.sha256(source.read_bytes()).hexdigest()[:12]
    out = CACHE_ROOT / "bin" / f"{source.stem}-{digest}"
    if out.is_file():
        return out
    out.parent.mkdir(parents=True, exist_ok=True)
    tmp = out.with_name(f"{out.name}.tmp-{os.getpid()}")
    try:
        proc = subprocess.run(
            ["swiftc", "-O", str(source), "-o", str(tmp)], capture_output=True, text=True
        )
    except FileNotFoundError:
        raise EnvBlocked("swiftc not found (install the Xcode command line tools)")
    if proc.returncode != 0:
        tmp.unlink(missing_ok=True)
        raise ProbeError(f"swiftc failed on {source.name}:\n{proc.stderr.strip()}")
    os.replace(tmp, out)
    if source == _OCR_SWIFT:
        _warm_ocr(out)
    return out


def _tiny_png(path: Path, w: int = 64, h: int = 32):
    """Write a plain white RGB PNG with nothing but zlib (no imaging module)."""
    import struct
    import zlib

    def chunk(tag, data):
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data))

    raw = b"".join(b"\x00" + b"\xff" * (3 * w) for _ in range(h))
    path.write_bytes(
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw))
        + chunk(b"IEND", b"")
    )


# Measured on macOS 27: the first Vision run of a newly written binary spends 40-70 s
# compiling its text model for the Neural Engine, and the very first attempt can
# even fail (e5rtError ... 13) before a retry succeeds; later runs take ~0.3 s. The
# compiled model is cached per executable path, so the helper is warmed once, at its
# final path, right after it is compiled, instead of inside some scenario's ocr step.
_OCR_TIMEOUT_S = 180.0


def _warm_ocr(binary: Path):
    png = binary.with_name(f"warm-{os.getpid()}.png")
    _tiny_png(png)
    print(f"probe: warming {binary.name} (first Vision run compiles its model, ~1 min)", flush=True)
    try:
        for _ in range(3):
            try:
                if subprocess.run([str(binary), str(png)], capture_output=True,
                                  timeout=_OCR_TIMEOUT_S).returncode == 0:
                    return
            except subprocess.TimeoutExpired:
                pass
    finally:
        png.unlink(missing_ok=True)
    # Not fatal: the ocr step retries and reports what Vision says.


def _run_helper(source: Path, *args, timeout: float = 60.0, attempts: int = 1) -> object:
    cmd = [str(_helper(source)), *map(str, args)]
    err = ""
    for _ in range(attempts):
        try:
            proc = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout)
        except subprocess.TimeoutExpired:
            err = f"timed out after {timeout:.0f}s"
            continue
        if proc.returncode == 0:
            return json.loads(proc.stdout)
        err = proc.stderr.strip()
    raise ProbeError(f"{source.stem} {' '.join(map(str, args))}: {err}")


def quartz(*args) -> object:
    return _run_helper(_QUARTZ_SWIFT, *args)


# ── instances (O1: own data dir, sockets and fixtures; O4: no real hotkeys) ──


def _tmpdir() -> str:
    # The app finds its socket dir through Rust's std::env::temp_dir(), i.e. $TMPDIR.
    # The probe passes this exact value down, so both sides agree on the path.
    return os.environ.get("TMPDIR") or "/tmp"


_INSTANCE_NAME_RE = re.compile(r"[^A-Za-z0-9_-]")


def instance_name(label: str, pid: int = None) -> str:
    """A UBILINK_INSTANCE value for `label`: unique per probe process, and inside the
    app's `[A-Za-z0-9_-]{1,32}` rule (automation/socket.rs) so it is never refused."""
    pid = os.getpid() if pid is None else pid
    clean = _INSTANCE_NAME_RE.sub("-", label)
    return f"p{pid}-{clean}"[:32]


class Instance:
    """One isolated ubilink-next process.

    Isolation (O1): its own UBILINK_INSTANCE (so its own automation socket), its own
    single-instance socket (UBILINK_INSTANCE_SOCKET, so it neither signals nor is
    signalled by the user's app or another instance), its own data dir
    (UBILINK_DATA_DIR, which desktop-gpui's `paths::data_dir()` honours in every
    build) and its own copy of desktop-gpui/probes/fixtures, all under one temp root.
    No real hotkeys (O4): UBILINK_SKIP_HOTKEYS=1. `restart()` keeps the temp root
    (the data must survive a restart) and replays every fixture loaded so far, since
    fixtures live in the app's memory.

    `release=True` runs the stripped release build (`build_binary(release=True)`)
    with the same isolation. It has no automation channel, so it is "up" once its
    single-instance socket exists, and `call()` refuses; it exists for probes that
    measure the shipped app (Phase 1 R15) rather than drive it.
    """

    def __init__(self, label: str, binary, release: bool = False):
        self.label = label
        self.name = instance_name(label)
        # A Build (normal use) or a plain path (tests, ad-hoc use).
        self.binary = Path(binary.path if isinstance(binary, Build) else binary)
        self.release = release
        self.root = None
        self.proc = None
        self._sock = None
        self._reader = None
        self._fixtures = {}

    # Paths --------------------------------------------------------------
    @property
    def socket_path(self) -> Path:
        return Path(_tmpdir()) / f"ubilink-{os.getuid()}" / self.name / "auto.sock"

    @property
    def data_dir(self) -> Path:
        return self.root / "data"

    @property
    def fixtures_dir(self) -> Path:
        return self.root / "fixtures"

    @property
    def log_path(self) -> Path:
        return self.root / "app.log"

    @property
    def si_socket_path(self) -> Path:
        return self.root / "si.sock"

    # Lifecycle ----------------------------------------------------------
    def prepare(self):
        """Create the temp root (data dir, fixture copy) without launching."""
        if self.root is not None:
            return
        self.root = Path(tempfile.mkdtemp(prefix=f"ubprobe-{self.name}-", dir=_tmpdir()))
        self.data_dir.mkdir()
        src_fixtures = PROBES_DIR / "fixtures"
        if src_fixtures.is_dir():
            # A private copy: a scenario may rewrite a fixture mid-run (onboarding
            # flips a permission state) without touching the repo or a sibling.
            shutil.copytree(src_fixtures, self.fixtures_dir)
        else:
            self.fixtures_dir.mkdir()

    def env(self) -> dict:
        # Drop any UBILINK_* the caller's shell exported: an instance must be
        # configured only by what is set here.
        env = {k: v for k, v in os.environ.items() if not k.startswith("UBILINK_")}
        env.update(
            UBILINK_INSTANCE=self.name,
            UBILINK_SKIP_HOTKEYS="1",
            UBILINK_DATA_DIR=str(self.data_dir),
            UBILINK_INSTANCE_SOCKET=str(self.si_socket_path),
            TMPDIR=_tmpdir(),
        )
        return env

    def start(self, timeout: float = _LAUNCH_TIMEOUT_S):
        self.prepare()
        self._launch(timeout)

    def _launch(self, timeout: float):
        # Launched directly, not through `open`: LaunchServices would fold it into the
        # user's running ubilink-next (same bundle id) instead of starting an instance.
        log = open(self.log_path, "ab")
        try:
            self.proc = subprocess.Popen(
                [str(self.binary)],
                env=self.env(),
                cwd=self.root,
                stdin=subprocess.DEVNULL,
                stdout=log,
                stderr=subprocess.STDOUT,
                start_new_session=True,
            )
        finally:
            log.close()
        # Ready = the automation socket answers; a release build has none, so its
        # single-instance socket (created before GPUI starts) is the signal instead.
        ready_on = self.si_socket_path if self.release else self.socket_path
        deadline = time.monotonic() + timeout
        while True:
            rc = self.proc.poll()
            if rc is not None:
                raise ProbeError(
                    f"instance {self.name} exited {rc} before serving {ready_on}\n"
                    + self.log_tail()
                )
            if self.release:
                if ready_on.exists():
                    return
            else:
                try:
                    self._connect()
                    break
                except OSError:
                    pass
            if time.monotonic() > deadline:
                raise ProbeError(
                    f"instance {self.name} did not serve {ready_on} within {timeout:.0f}s "
                    + ("" if self.release else "(is this an --features automation build?)")
                    + "\n" + self.log_tail()
                )
            time.sleep(0.05)
        for kind, path in list(self._fixtures.items()):
            self.call("load_fixture", kind=kind, path=path)

    def _connect(self):
        s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        try:
            s.settimeout(_CALL_TIMEOUT_S)
            s.connect(str(self.socket_path))
        except OSError:
            s.close()
            raise
        self._sock = s
        self._reader = s.makefile("rb")

    def log_tail(self, lines: int = 15) -> str:
        try:
            text = self.log_path.read_text(errors="replace").splitlines()
        except OSError:
            return "  (no app log)"
        return "\n".join(f"  | {line}" for line in text[-lines:])

    def request(self, verb: str, args: dict = None) -> dict:
        """Send one request and return the raw response object."""
        if self.release:
            raise ProbeError(f"instance {self.name} is a release build: it has no automation channel")
        if self._sock is None:
            raise ProbeError(f"instance {self.name} is not running")
        line = json.dumps({"verb": verb, "args": args or {}}) + "\n"
        try:
            self._sock.sendall(line.encode())
            reply = self._reader.readline()
        except OSError as e:
            raise ProbeError(f"instance {self.name}: {verb}: {e}\n{self.log_tail()}")
        if not reply:
            raise ProbeError(f"instance {self.name} closed the socket on {verb}\n{self.log_tail()}")
        resp = json.loads(reply)
        if verb == "load_fixture" and resp.get("ok"):
            self._fixtures[(args or {}).get("kind")] = (args or {}).get("path")
        return resp

    def call(self, verb: str, **args):
        resp = self.request(verb, args)
        if not resp.get("ok"):
            raise CallError(resp.get("error"))
        return resp.get("result")

    def restart(self, timeout: float = _LAUNCH_TIMEOUT_S):
        self._terminate()
        self._launch(timeout)

    def _terminate(self):
        if self._reader is not None:
            self._reader.close()
        if self._sock is not None:
            self._sock.close()
        self._sock = self._reader = None
        if self.proc is not None and self.proc.poll() is None:
            self.proc.send_signal(signal.SIGTERM)
            try:
                self.proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait(timeout=5)
        # The app does not unlink its socket on exit; remove it so a restart's wait
        # never mistakes the stale file for the new process.
        with contextlib.suppress(OSError):
            self.socket_path.unlink()
        if self.root is not None:
            with contextlib.suppress(OSError):
                self.si_socket_path.unlink()

    def stop(self):
        self._terminate()
        with contextlib.suppress(OSError):
            self.socket_path.parent.rmdir()  # this instance's 0700 dir
        if self.root is not None:
            shutil.rmtree(self.root, ignore_errors=True)
            self.root = None

    def __enter__(self):
        self.start()
        return self

    def __exit__(self, *exc):
        self.stop()
        return False


# ── desktop lock (O4: "GUI probes hold a desktop lock") ─────────────────────


@contextlib.contextmanager
def file_lock(path: Path, what: str, timeout: float = _LOCK_TIMEOUT_S, out=print):
    """Hold an exclusive flock on `path` for the duration; print who holds it if we wait.

    flock is released by the kernel if the holder dies, so a crashed probe never
    wedges the next one. The holder writes its pid into the file for that message.
    """
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    fd = os.open(path, os.O_RDWR | os.O_CREAT, 0o600)
    try:
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            holder = os.pread(fd, 64, 0).decode(errors="replace").strip() or "?"
            out(f"probe: waiting for the {what} {path} (held by pid {holder})")
            deadline = time.monotonic() + timeout
            while True:
                try:
                    fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
                    break
                except BlockingIOError:
                    if time.monotonic() > deadline:
                        raise ProbeError(f"{what} {path} still held after {timeout:.0f}s")
                    time.sleep(0.25)
        os.ftruncate(fd, 0)
        os.pwrite(fd, f"{os.getpid()}\n".encode(), 0)
        yield
    finally:
        with contextlib.suppress(OSError):
            fcntl.flock(fd, fcntl.LOCK_UN)
        os.close(fd)


def desktop_lock(path: Path = None, timeout: float = _LOCK_TIMEOUT_S, out=print):
    """The desktop lock (O4: "GUI probes hold a desktop lock").

    The pasteboard, the key window and the cursor are one per desktop. Two probes
    (parallel Phase 1 tasks, TN4 of the Phase 1 spec) opening panels at once would
    steal key status from each other and race on the pasteboard, so every `run`
    holds this lock around its scenarios.
    """
    return file_lock(path or CACHE_ROOT / "desktop.lock", "desktop lock", timeout, out)


# ── matching helpers (pure; covered by scripts/test_probe.py) ───────────────

_MISSING = object()


def get_path(obj, path: str):
    """Follow a dot path ("shell.window_kind", "items.0.uri", "" for the root)."""
    if path in ("", "."):
        return obj
    for part in path.split("."):
        if isinstance(obj, list):
            try:
                obj = obj[int(part)]
            except (ValueError, IndexError):
                return _MISSING
        elif isinstance(obj, dict):
            if part not in obj:
                return _MISSING
            obj = obj[part]
        else:
            return _MISSING
    return obj


_TYPE_NAMES = {
    "list": list, "object": dict, "string": str, "bool": bool, "null": type(None),
}


def _type_ok(value, name: str) -> bool:
    if name == "number":
        return isinstance(value, (int, float)) and not isinstance(value, bool)
    return isinstance(value, _TYPE_NAMES[name])


def _num(v) -> bool:
    return isinstance(v, (int, float)) and not isinstance(v, bool)


# Operator name -> (actual, operand) -> bool. An expectation object whose keys are
# ALL operator names is a matcher; anything else is compared for equality.
_OPS = {
    "eq": lambda a, b: a == b,
    "ne": lambda a, b: a != b,
    "in": lambda a, b: a in b,
    "contains": lambda a, b: isinstance(a, (str, list)) and b in a,
    "not_contains": lambda a, b: isinstance(a, (str, list)) and b not in a,
    "text": lambda a, b: isinstance(a, str) and text_matches(b, a),
    "len": lambda a, b: hasattr(a, "__len__") and len(a) == b,
    "min_len": lambda a, b: hasattr(a, "__len__") and len(a) >= b,
    "max_len": lambda a, b: hasattr(a, "__len__") and len(a) <= b,
    "lt": lambda a, b: _num(a) and a < b,
    "le": lambda a, b: _num(a) and a <= b,
    "gt": lambda a, b: _num(a) and a > b,
    "ge": lambda a, b: _num(a) and a >= b,
    "type": lambda a, b: _type_ok(a, b),
    "matches": lambda a, b: isinstance(a, str) and re.search(b, a) is not None,
    "exists": lambda a, b: (a is not _MISSING) == b,
}


def is_matcher(spec) -> bool:
    return isinstance(spec, dict) and bool(spec) and all(k in _OPS for k in spec)


def validate_matcher(spec, where: str):
    if is_matcher(spec):
        for op in ("text", "contains", "not_contains"):
            # "" is a substring of everything: the check would pass vacuously.
            if op in spec and spec[op] == "":
                raise ScenarioError(f"{where}: {op} needs a non-empty operand")
        if "type" in spec and spec["type"] not in (*_TYPE_NAMES, "number"):
            raise ScenarioError(f"{where}: unknown type {spec['type']!r}")
        if "matches" in spec:
            try:
                re.compile(spec["matches"])
            except re.error as e:
                raise ScenarioError(f"{where}: bad regex {spec['matches']!r}: {e}")


def match_value(actual, spec) -> bool:
    if is_matcher(spec):
        if actual is _MISSING:
            return spec.get("exists") is False and len(spec) == 1
        return all(_OPS[op](actual, operand) for op, operand in spec.items())
    return actual is not _MISSING and actual == spec


def check_expect(result, expect: dict) -> list:
    """Return [(path, expected, actual)] for every expectation `result` misses."""
    misses = []
    for path, spec in expect.items():
        actual = get_path(result, path)
        if not match_value(actual, spec):
            misses.append((path, spec, "<missing>" if actual is _MISSING else actual))
    return misses


def norm_text(s: str) -> str:
    # NFKC folds "…" into "..." and full-width forms into ASCII; casefold and
    # whitespace collapse make "Capture  link" and "capture link" the same text.
    s = unicodedata.normalize("NFKC", str(s)).casefold()
    return " ".join(s.split())


def text_matches(want: str, have: str, exact: bool = False) -> bool:
    """Contains-normalised match that tolerates a UI's truncation with an ellipsis.

    `want` matches `have` when, after normalising both, `want` is a substring of
    `have` (or equal, with exact=True). If `have` was cut with a trailing ellipsis
    ("Capture: https://exa…"), it also matches when the visible text before the
    ellipsis ends with a prefix of `want` at least 6 characters long (or all of
    `want`, if shorter); a leading ellipsis is handled the same way from the other
    end. Rows in a narrow panel are truncated by design, and a probe must not fail
    because the fixture URI is longer than the row.
    """
    w, h = norm_text(want), norm_text(have)
    if not w:
        # Empty text would be a substring of everything: never a match (scenario
        # validation refuses it too, so this only guards direct callers).
        return False
    if (h == w) if exact else (w in h):
        return True
    need = min(len(w), 6)
    if h.endswith("..."):
        head = h[:-3].rstrip()
        if exact:
            if len(head) >= need and w.startswith(head):
                return True
        else:
            for i in range(len(head)):
                tail = head[i:]
                if len(tail) >= need and w.startswith(tail):
                    return True
    if h.startswith("..."):
        rest = h[3:].lstrip()
        if exact:
            if len(rest) >= need and w.endswith(rest):
                return True
        else:
            for j in range(len(rest), 0, -1):
                head = rest[:j]
                if len(head) >= need and w.endswith(head):
                    return True
    return False


def _norm_role(role) -> str:
    return re.sub(r"[^a-z0-9]", "", str(role or "").lower())


# What gpui-pre 0.3.7's debug_a11y_tree_json puts in a node's `aria` object
# (src/window/a11y/debug.rs, node_to_json), split by what a probe does with it.
# Text a user can read: these are what `text` matches.
A11Y_TEXT_FIELDS = ("label", "value", "description", "placeholder", "tooltip")
# State a probe can assert with `state`. `toggled` is emitted as "True"/"False"/
# "Mixed" and normalised to true/false/"mixed"; `on_action` is exposed as
# `actions`; `focused` is derived from the dump's gpui_focus/active_descendant_focus
# ids. There is no disabled flag in 0.3.7's dump, so `disabled` cannot be asserted.
A11Y_STATE_FIELDS = (
    "selected", "expanded", "toggled", "orientation", "focused", "actions",
    "numeric_value", "min_numeric_value", "max_numeric_value", "numeric_value_step",
    "level", "position_in_set", "size_of_set",
    "row_index", "column_index", "row_count", "column_count",
)


def a11y_nodes(tree) -> list:
    """Flatten a debug_a11y_tree_json dump into nodes in document order.

    Each node is {"id", "role", "texts", "state"}. Order is a depth-first walk from
    the dump's `root` through `children` (the order the view laid them out), not the
    dump's key order: the ephemeral ids follow AccessKit's update order (a parent
    can come after its children) and a JSON object's key order is not guaranteed.
    Nodes the walk does not reach are appended in key order.
    """
    if not isinstance(tree, dict):
        return []
    nodes = tree.get("nodes") or {}
    if isinstance(nodes, list):
        nodes = {str(i): n for i, n in enumerate(nodes)}
    focus = {tree.get("gpui_focus"), tree.get("active_descendant_focus")} - {None}

    order, seen = [], set()
    stack = [tree.get("root")] if tree.get("root") in nodes else []
    while stack:
        nid = stack.pop()
        if nid in seen or nid not in nodes:
            continue
        seen.add(nid)
        order.append(nid)
        kids = nodes[nid].get("children") or [] if isinstance(nodes[nid], dict) else []
        stack.extend(reversed(kids))
    order += [k for k in nodes if k not in seen]

    out = []
    for nid in order:
        node = nodes[nid]
        if not isinstance(node, dict):
            continue
        aria = node.get("aria") or {}
        texts = [aria[k] for k in A11Y_TEXT_FIELDS if isinstance(aria.get(k), str)]
        state = {k: aria[k] for k in A11Y_STATE_FIELDS if k in aria}
        if "toggled" in state:
            t = str(state["toggled"]).lower()
            state["toggled"] = {"true": True, "false": False}.get(t, t)
        if "on_action" in aria:
            state["actions"] = aria["on_action"]
        state["focused"] = nid in focus
        out.append({"id": nid, "role": aria.get("role"), "texts": texts, "state": state})
    return out


def _node_matches(node, role=None, text=None, state=None, exact=False) -> bool:
    if role is not None and _norm_role(node["role"]) != _norm_role(role):
        return False
    if text is not None and not any(text_matches(text, t, exact) for t in node["texts"]):
        return False
    # A state the node does not report counts as null: `false` does not match an
    # unselectable node, `{"ne": true}` does.
    for k, spec in (state or {}).items():
        if not match_value(node["state"].get(k), spec):
            return False
    return True


def find_nodes(tree, role=None, text=None, exact=False, state=None) -> list:
    return [n for n in a11y_nodes(tree) if _node_matches(n, role, text, state, exact)]


def _describe_node(n) -> str:
    flags = {k: v for k, v in n["state"].items() if k not in ("actions",) and v not in (None, False)}
    return f"{n['role']}: {' | '.join(n['texts'])}" + (f" {flags}" if flags else "")


def check_a11y_query(tree, q: dict, absent=False, exact=False, counts=None):
    """Check one `see` query against a tree; raise StepFailed with what was seen.

    Three shapes (validated by validate_step): a node query {role?, text?, state?}
    (present, absent, or counted), an index query {role, nth, text?, state?} (the
    nth node of that role, 0-based, negative from the end, must match), and a
    sequence query {sequence: {role, texts, mode}} over that role's nodes in
    document order.
    """
    nodes = a11y_nodes(tree)
    seen = [_describe_node(n) for n in nodes][:60]
    if "sequence" in q:
        seq = q["sequence"]
        got = [n for n in nodes if _node_matches(n, role=seq["role"])]
        texts, mode = seq["texts"], seq.get("mode", "prefix")

        def m(want, node):
            return any(text_matches(want, t, exact) for t in node["texts"])

        if mode == "ordered":
            i = 0
            for node in got:
                if i < len(texts) and m(texts[i], node):
                    i += 1
            ok = i == len(texts)
        else:
            ok = len(got) >= len(texts) and all(m(w, n) for w, n in zip(texts, got))
            if mode == "exact":
                ok = ok and len(got) == len(texts)
        if not ok:
            raise StepFailed(
                f"{seq['role']} sequence ({mode}) mismatch",
                texts,
                [" | ".join(n["texts"]) for n in got][:60],
            )
        return
    if "nth" in q:
        of_role = [n for n in nodes if _node_matches(n, role=q["role"])]
        k = q["nth"]
        if not -len(of_role) <= k < len(of_role):
            raise StepFailed(
                f"no {q['role']} node #{k}", f"at least {k + 1 if k >= 0 else -k} {q['role']} node(s)",
                [_describe_node(n) for n in of_role],
            )
        node = of_role[k]
        if not _node_matches(node, None, q.get("text"), q.get("state"), exact):
            raise StepFailed(f"{q['role']} node #{k} mismatch", _short(q), _describe_node(node))
        return
    hits = [n for n in nodes if _node_matches(n, q.get("role"), q.get("text"), q.get("state"), exact)]
    n = len(hits)
    counts = counts or {}
    bad = (
        (absent and n > 0)
        or (not absent and n == 0)
        or ("count" in counts and n != counts["count"])
        or ("min_count" in counts and n < counts["min_count"])
        or ("max_count" in counts and n > counts["max_count"])
    )
    if bad:
        want = ("no node like " if absent else "node like ") + _short(q)
        for k, v in counts.items():
            want += f" ({k} {v}, found {n})"
        raise StepFailed("a11y tree mismatch", want, seen)


def _srgb_to_lab(rgb):
    def lin(c):
        c /= 255.0
        return c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4

    r, g, b = (lin(float(c)) for c in rgb)
    x = (0.4124564 * r + 0.3575761 * g + 0.1804375 * b) / 0.95047
    y = 0.2126729 * r + 0.7151522 * g + 0.0721750 * b
    z = (0.0193339 * r + 0.1191920 * g + 0.9503041 * b) / 1.08883

    def f(t):
        return t ** (1 / 3) if t > 216 / 24389 else (24389 / 27 * t + 16) / 116

    fx, fy, fz = f(x), f(y), f(z)
    return 116 * fy - 16, 500 * (fx - fy), 200 * (fy - fz)


def parse_hex(color: str):
    m = re.fullmatch(r"#?([0-9a-fA-F]{6})", color.strip())
    if not m:
        raise ScenarioError(f"not a #rrggbb colour: {color!r}")
    v = m.group(1)
    return tuple(int(v[i:i + 2], 16) for i in (0, 2, 4))


def delta_e(rgb1, rgb2) -> float:
    """CIE76 ΔE between two sRGB colours (D65). ~2.3 is a just-noticeable difference."""
    a, b = _srgb_to_lab(rgb1), _srgb_to_lab(rgb2)
    return sum((p - q) ** 2 for p, q in zip(a, b)) ** 0.5


def rect_within(inner: dict, outer: dict, slack: float = 0.5) -> bool:
    return (
        inner["x"] >= outer["x"] - slack
        and inner["y"] >= outer["y"] - slack
        and inner["x"] + inner["w"] <= outer["x"] + outer["w"] + slack
        and inner["y"] + inner["h"] <= outer["y"] + outer["h"] + slack
    )


def order_displays(displays: list) -> list:
    """Display index order for scenarios: the main display is 0, the rest left to right."""
    return sorted(displays, key=lambda d: (not d.get("main"), d["bounds"]["x"], d["bounds"]["y"]))


# ── scenario files ────────────────────────────────────────────────────────

# Steps that only read state, so `retry_ms` may re-issue them until they pass.
_READ_ONLY_VERBS = {"state", "metrics", "window_id", "a11y", "opened"}

# Step type -> the option keys it accepts besides its own key. Unknown types and
# unknown options are scenario errors, so a typo ("expcet") can never make a
# step vacuously pass.
STEP_TYPES = {
    "call": {"args", "expect", "expect_error", "retry_ms"},
    "see": {"absent", "count", "min_count", "max_count", "exact", "timeout_ms", "route"},
    "ocr": {"absent", "exact"},
    "pixel": {"color", "delta_e"},
    "pasteboard": {"expect", "changed"},
    "graph": {"db", "export", "links", "no_links", "link_count", "resource_count", "timeout_ms"},
    "file": {"exists", "expect", "timeout_ms"},
    "winframe": {"timeout_ms"},
    "warp_cursor": set(),
    "activate_other": {"settle_ms"},
    "restart": set(),
    "sleep_ms": set(),
}
# `block`: consecutive steps with the same block id are one iteration, the unit a
# focus-theft retry re-runs (see `run_steps`).
_COMMON_OPTIONS = {"name", "block"}
_SCENARIO_KEYS = {"description", "requirement", "notes", "setup", "steps"}
_SETUP_KEYS = {"fixtures", "files"}


def step_type(step, where: str) -> str:
    if not isinstance(step, dict):
        raise ScenarioError(f"{where}: a step must be an object, got {type(step).__name__}")
    kinds = [k for k in step if k in STEP_TYPES]
    if len(kinds) != 1:
        raise ScenarioError(
            f"{where}: unknown step {sorted(step)} (each step needs exactly one of: "
            + ", ".join(STEP_TYPES) + ")"
        )
    kind = kinds[0]
    extra = set(step) - {kind} - STEP_TYPES[kind] - _COMMON_OPTIONS
    if extra:
        raise ScenarioError(f"{where}: unknown option(s) {sorted(extra)} for a {kind} step")
    return kind


def _nonblank(v) -> bool:
    return isinstance(v, str) and bool(v.strip())


_SEQUENCE_MODES = ("prefix", "exact", "ordered")


def _validate_state(state, where: str):
    if not isinstance(state, dict) or not state:
        raise ScenarioError(f"{where}: state must be a non-empty object of flag expectations")
    for k, spec in state.items():
        if k == "disabled":
            raise ScenarioError(
                f"{where}: gpui-pre 0.3.7's a11y dump has no disabled flag, so it cannot be asserted"
            )
        if k not in A11Y_STATE_FIELDS:
            raise ScenarioError(f"{where}: unknown a11y state {k!r} (one of {', '.join(A11Y_STATE_FIELDS)})")
        validate_matcher(spec, f"{where}: state[{k!r}]")


def _validate_see(step, where: str):
    v = step["see"]
    queries = v if isinstance(v, list) else [v]
    if not queries:
        raise ScenarioError(f"{where}: see needs at least one query")
    counted = any(k in step for k in ("count", "min_count", "max_count"))
    if "route" in step and not _nonblank(step["route"]):
        raise ScenarioError(f"{where}: route must be a route name")
    for q in queries:
        if not isinstance(q, dict) or not q:
            raise ScenarioError(f"{where}: each see query is an object")
        if "sequence" in q:
            seq = q["sequence"]
            if set(q) != {"sequence"} or not isinstance(seq, dict) or not set(seq) <= {"role", "texts", "mode"}:
                raise ScenarioError(f"{where}: a sequence query is {{sequence: {{role, texts, mode}}}} alone")
            if not _nonblank(seq.get("role")):
                raise ScenarioError(f"{where}: sequence needs a role")
            texts = seq.get("texts")
            if not isinstance(texts, list) or not texts or not all(_nonblank(t) for t in texts):
                raise ScenarioError(f"{where}: sequence texts must be a non-empty list of non-empty strings")
            if seq.get("mode", "prefix") not in _SEQUENCE_MODES:
                raise ScenarioError(f"{where}: sequence mode is one of {', '.join(_SEQUENCE_MODES)}")
            if step.get("absent") or counted:
                raise ScenarioError(f"{where}: absent/count do not apply to a sequence query")
            continue
        extra = set(q) - {"role", "text", "state", "nth"}
        if extra:
            raise ScenarioError(f"{where}: unknown see query key(s) {sorted(extra)}")
        if "role" in q and not _nonblank(q["role"]):
            raise ScenarioError(f"{where}: role must be a non-empty string")
        if "text" in q and not _nonblank(q["text"]):
            raise ScenarioError(f"{where}: text must be non-empty (empty text matches every node)")
        if "role" not in q and "text" not in q:
            raise ScenarioError(f"{where}: a see query needs a role or a text")
        if "state" in q:
            _validate_state(q["state"], where)
        if "nth" in q:
            if not (isinstance(q["nth"], int) and not isinstance(q["nth"], bool)) or "role" not in q:
                raise ScenarioError(f"{where}: nth is an integer index into the nodes of `role`")
            if step.get("absent") or counted:
                raise ScenarioError(f"{where}: absent/count do not apply to an nth query")


def validate_step(step, where: str) -> str:
    kind = step_type(step, where)
    v = step[kind]
    if kind == "call":
        if not isinstance(v, str):
            raise ScenarioError(f"{where}: call takes a verb name")
        if "retry_ms" in step and v not in _READ_ONLY_VERBS:
            raise ScenarioError(f"{where}: retry_ms only on read-only verbs {sorted(_READ_ONLY_VERBS)}")
        if "args" in step and not isinstance(step["args"], dict):
            raise ScenarioError(f"{where}: args must be an object")
        for path, spec in (step.get("expect") or {}).items():
            validate_matcher(spec, f"{where}: expect[{path!r}]")
    elif kind == "see":
        _validate_see(step, where)
    elif kind == "ocr":
        texts = v if isinstance(v, list) else [v]
        if not texts or not all(_nonblank(t) for t in texts):
            raise ScenarioError(f"{where}: ocr takes a non-empty string or non-empty list of them")
    elif kind == "pixel":
        if not isinstance(v, dict) or not {"x", "y"} <= set(v) or not set(v) <= {"x", "y", "w", "h"}:
            raise ScenarioError(f"{where}: pixel takes {{x, y[, w, h]}} in window points")
        if "color" not in step:
            raise ScenarioError(f"{where}: pixel needs color")
        parse_hex(step["color"])
    elif kind == "pasteboard":
        if v not in ("set_nonce", "read"):
            raise ScenarioError(f"{where}: pasteboard is set_nonce or read")
        if v == "read" and "expect" not in step and "changed" not in step:
            raise ScenarioError(f"{where}: pasteboard read needs expect or changed")
        if "expect" in step:
            validate_matcher(step["expect"], f"{where}: expect")
    elif kind == "graph":
        if v is not True:
            raise ScenarioError(f"{where}: graph takes true (options say what to check)")
        if not any(k in step for k in ("links", "no_links", "link_count", "resource_count")):
            raise ScenarioError(f"{where}: graph needs links, no_links, link_count or resource_count")
        for k in ("links", "no_links"):
            if k in step:
                pairs = step[k]
                ok = isinstance(pairs, list) and pairs and all(
                    isinstance(pr, list) and len(pr) == 2 and all(_nonblank(u) for u in pr)
                    for pr in pairs
                )
                if not ok:
                    raise ScenarioError(f"{where}: graph {k} must be a non-empty list of [uriA, uriB]")
        for k in ("link_count", "resource_count"):
            if k in step and not (isinstance(step[k], int) and not isinstance(step[k], bool) and step[k] >= 0):
                raise ScenarioError(f"{where}: graph {k} must be a non-negative integer")
    elif kind == "file":
        if not _nonblank(v):
            raise ScenarioError(f"{where}: file takes a path")
        for path, spec in (step.get("expect") or {}).items():
            validate_matcher(spec, f"{where}: expect[{path!r}]")
    elif kind == "winframe":
        if not (v in ("any", "cursor", "state", "gone") or (isinstance(v, int) and not isinstance(v, bool))):
            raise ScenarioError(f"{where}: winframe is any, cursor, state, gone or a display index")
    elif kind == "warp_cursor":
        ok = isinstance(v, int) and not isinstance(v, bool) or (
            isinstance(v, dict) and "display" in v and set(v) <= {"display", "x", "y"}
        )
        if not ok:
            raise ScenarioError(f"{where}: warp_cursor takes a display index or {{display, x, y}}")
    elif kind == "activate_other":
        # O4: probes never launch the user's apps; the probe's own activator is the
        # only "other app" there is.
        if v is not True:
            raise ScenarioError(f"{where}: activate_other takes true (the probe's own activator app)")
    elif kind == "restart":
        if v is not True:
            raise ScenarioError(f"{where}: restart takes true")
    elif kind == "sleep_ms":
        if not _num(v) or v < 0:
            raise ScenarioError(f"{where}: sleep_ms takes a non-negative number")
    return kind


def load_scenario(name: str, probes_dir: Path = None) -> dict:
    """Load and fully validate a scenario, before any instance is launched."""
    probes_dir = probes_dir or PROBES_DIR
    path = Path(name) if name.endswith(".json") else probes_dir / f"{name}.json"
    if not path.is_file():
        raise ScenarioError(f"no scenario {name!r} ({path} not found)")
    try:
        sc = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as e:
        raise ScenarioError(f"{path}: bad JSON: {e}")
    if not isinstance(sc, dict):
        raise ScenarioError(f"{path}: top level must be an object")
    extra = set(sc) - _SCENARIO_KEYS
    if extra:
        raise ScenarioError(f"{path}: unknown key(s) {sorted(extra)}")
    setup = sc.get("setup", {})
    if not isinstance(setup, dict) or set(setup) - _SETUP_KEYS:
        raise ScenarioError(f"{path}: setup takes only {sorted(_SETUP_KEYS)}")
    steps = sc.get("steps")
    if not isinstance(steps, list) or not steps:
        raise ScenarioError(f"{path}: steps must be a non-empty list")
    for i, step in enumerate(steps, 1):
        validate_step(step, f"{path.name} step {i}")
    sc["_name"] = path.stem
    sc["_path"] = path
    return sc


def expand(obj, tokens: dict):
    """Replace {root}, {data_dir}, {fixtures}, {probes}, {nonce}, {instance} in strings."""
    if isinstance(obj, str):
        for k, v in tokens.items():
            obj = obj.replace("{" + k + "}", str(v))
        return obj
    if isinstance(obj, list):
        return [expand(x, tokens) for x in obj]
    if isinstance(obj, dict):
        return {expand(k, tokens): expand(v, tokens) for k, v in obj.items()}
    return obj


def _short(v, limit: int = 300) -> str:
    s = json.dumps(v, sort_keys=True, default=str) if not isinstance(v, str) else v
    return s if len(s) <= limit else s[: limit - 3] + "..."


# ── step runners ──────────────────────────────────────────────────────────


class RunContext:
    """Per-scenario, per-instance state the steps share."""

    def __init__(self, inst: Instance, scenario: dict):
        self.inst = inst
        self.scenario = scenario
        self.nonce = None
        self.pb_count = None
        self.shots = 0
        # Processes that may be frontmost without it counting as focus theft.
        # The instance's own pid is added at check time (`restart` changes it).
        self.own_pids = {os.getpid()}
        # The panel window a `winframe` step (or a `window_id` call) last found for
        # the current panel, for `winframe: "gone"`. Cleared whenever a call shows a
        # new panel appearing, so a `gone` never checks a previous panel's window.
        self.last_panel_wid = None
        self.panel_visible = False

    def owned_pids(self) -> set:
        proc = getattr(self.inst, "proc", None)
        return self.own_pids | ({proc.pid} if proc is not None else set())

    def observe(self, verb: str, result):
        """Track the panel across call results (see `last_panel_wid`)."""
        if verb == "window_id" and isinstance(result, dict) and result.get("window_id"):
            self.last_panel_wid = result["window_id"]
            self.panel_visible = True
            return
        self.last_panel_wid, self.panel_visible = track_panel(
            self.last_panel_wid, self.panel_visible, visible_in(result)
        )

    def tokens(self) -> dict:
        t = {
            "root": self.inst.root,
            "data_dir": self.inst.data_dir,
            "fixtures": self.inst.fixtures_dir,
            "probes": PROBES_DIR,
            "instance": self.inst.name,
        }
        if self.nonce is not None:
            t["nonce"] = self.nonce
        return t


def visible_in(result):
    """`shell.visible` from a call result: `state`/`trigger_hotkey`/`open_view` carry
    it at `shell`, `resign_key` at `state.shell`. None when the result has neither."""
    if not isinstance(result, dict):
        return None
    for shell in (result.get("shell"), (result.get("state") or {}).get("shell")):
        if isinstance(shell, dict) and isinstance(shell.get("visible"), bool):
            return shell["visible"]
    return None


def track_panel(wid, was_visible: bool, visible):
    """(tracked window, visible) after a call reporting `visible`. A hidden → shown
    transition is a new panel window, so the tracked number is dropped."""
    if visible is None:
        return wid, was_visible
    if visible and not was_visible:
        wid = None
    return wid, visible


def _poll(timeout_ms, attempt):
    """Run attempt() until it returns without StepFailed or the time is up."""
    deadline = time.monotonic() + (timeout_ms or 0) / 1000.0
    while True:
        try:
            return attempt()
        except StepFailed:
            if time.monotonic() >= deadline:
                raise
            time.sleep(0.1)


def run_call(ctx: RunContext, step: dict) -> str:
    verb = step["call"]
    args = expand(step.get("args", {}), ctx.tokens())
    expect = expand(step.get("expect", {}), ctx.tokens())
    want_error = step.get("expect_error")

    def attempt():
        resp = ctx.inst.request(verb, args)
        if want_error is not None:
            err = resp.get("error") or ""
            if resp.get("ok") or (want_error is not True and str(want_error) not in err):
                raise StepFailed(f"{verb} should fail", f"error containing {want_error!r}", resp)
            return f"error: {err}"
        if not resp.get("ok"):
            raise StepFailed(f"{verb} returned an error", "ok", resp.get("error"))
        ctx.observe(verb, resp.get("result"))
        misses = check_expect(resp.get("result"), expect)
        if misses:
            raise StepFailed(
                f"{verb}: {len(misses)} expectation(s) not met",
                {p: e for p, e, _ in misses},
                {p: a for p, _, a in misses},
            )
        return f"{len(expect)} expectation(s) met" if expect else "ok"

    return _poll(step.get("retry_ms"), attempt)


def run_see(ctx: RunContext, step: dict) -> str:
    queries = step["see"] if isinstance(step["see"], list) else [step["see"]]
    queries = expand(queries, ctx.tokens())
    absent = step.get("absent", False)
    exact = step.get("exact", False)
    counts = {k: step[k] for k in ("count", "min_count", "max_count") if k in step}

    def attempt():
        if "route" in step:
            route = ctx.inst.call("state").get("route")
            if route != step["route"]:
                raise StepFailed("wrong view", f"route {step['route']}", route)
        try:
            tree = ctx.inst.call("a11y")
        except CallError as e:
            raise StepFailed("a11y failed", "an accessibility tree", str(e))
        if absent and not any(_norm_role(n["role"]) != "window" for n in a11y_nodes(tree)):
            # An empty (or window-only) tree proves nothing is absent; it proves
            # nothing rendered. Make `absent` fail rather than pass vacuously.
            raise StepFailed(
                "absent needs a rendered view", "a tree with at least one non-Window node",
                [_describe_node(n) for n in a11y_nodes(tree)],
            )
        for q in queries:
            check_a11y_query(tree, q, absent, exact, counts)
        verdict = "absent" if absent else "matched"
        return f"{len(queries)} a11y quer{'y' if len(queries) == 1 else 'ies'} {verdict}"

    return _poll(step.get("timeout_ms", 3000), attempt)


def _screen_access():
    """Raise EnvBlocked unless this process may capture the screen (TN5, R9)."""
    if not quartz("screen-access").get("granted"):
        raise EnvBlocked(
            "Screen Recording is not granted to the process running probe.py "
            "(System Settings > Privacy & Security > Screen Recording); "
            "ocr/pixel steps need it, a11y steps do not"
        )


def _panel_window(ctx: RunContext):
    """(window number, bounds in points) of the instance's open panel."""
    try:
        wid = ctx.inst.call("window_id")["window_id"]
    except CallError as e:
        raise StepFailed("no panel window", "an open panel", str(e))
    for w in screen_windows(ctx.inst.proc.pid):
        if w["number"] == wid:
            ctx.last_panel_wid = wid
            return wid, w["bounds"]
    raise StepFailed("panel not on screen", f"window {wid} on screen", "absent from CGWindowList")


def _capture(ctx: RunContext):
    _screen_access()
    wid, bounds = _panel_window(ctx)
    ctx.shots += 1
    shot_dir = ctx.inst.root / "shots"
    shot_dir.mkdir(exist_ok=True)
    png = shot_dir / f"shot-{ctx.shots}.png"
    # -x: no shutter sound; -o: no window shadow, so the image is the window's own
    # pixels and point (0, 0) is its top-left corner.
    proc = subprocess.run(
        ["screencapture", "-x", "-o", f"-l{wid}", str(png)], capture_output=True, text=True
    )
    if proc.returncode != 0 or not png.is_file() or png.stat().st_size == 0:
        raise EnvBlocked(f"screencapture -l{wid} produced no image: {proc.stderr.strip()}")
    return png, bounds


def run_ocr(ctx: RunContext, step: dict) -> str:
    wants = step["ocr"] if isinstance(step["ocr"], list) else [step["ocr"]]
    wants = expand(wants, ctx.tokens())
    png, _ = _capture(ctx)
    out = _run_helper(_OCR_SWIFT, png, timeout=_OCR_TIMEOUT_S, attempts=2)
    lines = [line["text"] for line in out.get("lines", [])]
    joined = " ".join(lines)
    absent = step.get("absent", False)
    exact = step.get("exact", False)
    for w in wants:
        found = any(text_matches(w, line, exact) for line in lines) or (
            not exact and text_matches(w, joined)
        )
        if found == absent:
            raise StepFailed(
                "OCR mismatch", ("absent: " if absent else "present: ") + w, lines
            )
    return f"{len(wants)} OCR text(s) {'absent' if absent else 'present'} ({png.name})"


def run_pixel(ctx: RunContext, step: dict) -> str:
    region = step["pixel"]
    want = parse_hex(step["color"])
    limit = step.get("delta_e", 3.0)
    png, bounds = _capture(ctx)
    got = quartz(
        "pixel", png, region["x"], region["y"], region.get("w", 1), region.get("h", 1), bounds["w"]
    )
    de = delta_e(want, got["rgb"])
    if de > limit:
        raise StepFailed(
            f"colour off by ΔE {de:.1f} (limit {limit})", step["color"], got["hex"]
        )
    return f"{got['hex']} within ΔE {de:.1f} of {step['color']}"


# The user's pasteboard text, saved before the first nonce of the run and put back
# at the end (O4: a probe must leave the host as it found it). Text only: a richer
# pasteboard (an image, files) is not restored, which the README states.
_saved_pasteboard = None


def _pbcount() -> int:
    return quartz("pbcount")["change_count"]


def run_pasteboard(ctx: RunContext, step: dict) -> str:
    global _saved_pasteboard
    if step["pasteboard"] == "set_nonce":
        if _saved_pasteboard is None:
            _saved_pasteboard = subprocess.run(["pbpaste"], capture_output=True).stdout
        ctx.nonce = f"ubilink-probe-nonce-{os.getpid()}-{time.monotonic_ns()}"
        subprocess.run(["pbcopy"], input=ctx.nonce.encode(), check=True)
        ctx.pb_count = _pbcount()
        return f"pasteboard = {ctx.nonce} (change count {ctx.pb_count})"
    text = subprocess.run(["pbpaste"], capture_output=True).stdout.decode(errors="replace")
    count = _pbcount()
    if "expect" in step:
        spec = expand(step["expect"], ctx.tokens())
        if not match_value(text, spec):
            raise StepFailed("pasteboard text", spec, text)
    if "changed" in step:
        if ctx.pb_count is None:
            raise ScenarioError("pasteboard changed: needs an earlier set_nonce")
        if (count > ctx.pb_count) != step["changed"]:
            raise StepFailed(
                "pasteboard change count",
                f"{'>' if step['changed'] else '=='} {ctx.pb_count}",
                count,
            )
    ctx.pb_count = count
    return f"pasteboard {_short(text, 80)!r} (change count {count})"


def restore_pasteboard():
    global _saved_pasteboard
    if _saved_pasteboard is not None:
        subprocess.run(["pbcopy"], input=_saved_pasteboard)
        _saved_pasteboard = None


def read_graph(db: Path = None, export: Path = None):
    """(set of frozenset({uri_a, uri_b}) links, resource count) from a graph.

    `db` is ubilink-core's SQLite file (resources.id is the URI; links holds one row
    per unordered pair), opened read-only so a probe can never change the graph it
    checks. `export` is the JSON Settings > Export writes (ubilink.graph.v3:
    resources carry guids, links name their ends by guid).
    """
    if export is not None:
        doc = json.loads(Path(export).read_text(encoding="utf-8"))
        uri_of = {r["guid"]: r["uri"] for r in doc.get("resources", [])}
        links = {
            frozenset((uri_of.get(k["a"], k["a"]), uri_of.get(k["b"], k["b"])))
            for k in doc.get("links", [])
        }
        return links, len(uri_of)
    conn = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
    try:
        rows = conn.execute(
            "SELECT a.uri, b.uri FROM links l "
            "JOIN resources a ON a.id = l.lo_id JOIN resources b ON b.id = l.hi_id"
        ).fetchall()
        n = conn.execute("SELECT COUNT(*) FROM resources").fetchone()[0]
    finally:
        conn.close()
    return {frozenset(r) for r in rows}, n


def run_graph(ctx: RunContext, step: dict) -> str:
    step = expand(step, ctx.tokens())
    export = Path(step["export"]) if "export" in step else None
    db = Path(step.get("db") or ctx.inst.data_dir / "ubilink-graph.sqlite")

    def attempt():
        src = export or db
        if not src.is_file():
            raise StepFailed("no graph", f"a graph at {src}", "missing")
        try:
            links, n_res = read_graph(db=None if export else db, export=export)
        except (sqlite3.Error, ValueError, KeyError) as e:
            raise StepFailed("unreadable graph", str(src), str(e))
        for a, b in step.get("links", []):
            if frozenset((a, b)) not in links:
                raise StepFailed("missing link", f"{a} <-> {b}", sorted(map(sorted, links))[:20])
        for a, b in step.get("no_links", []):
            if frozenset((a, b)) in links:
                raise StepFailed("unexpected link", f"no {a} <-> {b}", "present")
        if "link_count" in step and len(links) != step["link_count"]:
            raise StepFailed("link count", step["link_count"], len(links))
        if "resource_count" in step and n_res != step["resource_count"]:
            raise StepFailed("resource count", step["resource_count"], n_res)
        return f"{len(links)} link(s), {n_res} resource(s) in {src.name}"

    return _poll(step.get("timeout_ms", 0), attempt)


def run_file(ctx: RunContext, step: dict) -> str:
    path = Path(expand(step["file"], ctx.tokens()))
    expect = expand(step.get("expect", {}), ctx.tokens())
    want_exists = step.get("exists", True)

    def attempt():
        if path.exists() != want_exists:
            raise StepFailed("file existence", f"exists={want_exists}", f"exists={path.exists()}")
        if not want_exists:
            return f"{path} absent"
        try:
            doc = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, ValueError) as e:
            raise StepFailed("file is not JSON", str(path), str(e))
        misses = check_expect(doc, expect)
        if misses:
            raise StepFailed(
                f"{path.name}: {len(misses)} expectation(s) not met",
                {p: e for p, e, _ in misses},
                {p: a for p, _, a in misses},
            )
        return f"{path.name}: {len(expect)} expectation(s) met"

    return _poll(step.get("timeout_ms", 0), attempt)


def displays_or_blocked(displays: list) -> list:
    """An empty active-display list means the display is asleep (or the session has
    no screen): the host cannot show a window, which is not a product failure."""
    if not displays:
        raise EnvBlocked("display asleep: CGGetActiveDisplayList reports no active display")
    return displays


def active_displays() -> list:
    return displays_or_blocked(quartz("displays"))


def screen_windows(pid: int) -> list:
    """The pid's on-screen windows. The helper fails when the window server returns
    no list at all; that is the environment, never an empty list (see `gone`)."""
    try:
        return quartz("windows", pid)
    except ProbeError as e:
        if "returned nil" in str(e):
            raise EnvBlocked(f"the window server returned no window list (display asleep?): {e}")
        raise


def _display_for(target, displays: list, ctx: RunContext) -> dict:
    ordered = order_displays(displays)
    if target == "cursor":
        c = quartz("cursor")
        for d in ordered:
            b = d["bounds"]
            if b["x"] <= c["x"] < b["x"] + b["w"] and b["y"] <= c["y"] < b["y"] + b["h"]:
                return d
        raise StepFailed("cursor on no display", "a display under the cursor", c)
    if target == "state":
        did = (ctx.inst.call("state").get("shell") or {}).get("display_id")
        for d in ordered:
            if d["id"] == did:
                return d
        raise StepFailed("state.shell.display_id is no active display", [d["id"] for d in ordered], did)
    if target >= len(ordered):
        raise EnvBlocked(f"needs display {target}, the host has {len(ordered)}")
    return ordered[target]


def window_gone(wid, windows: list) -> bool:
    """`winframe: "gone"`: the panel window an earlier step saw is not on screen. With
    no earlier window there is nothing to check, so that is never a pass."""
    if wid is None:
        raise StepFailed("winframe gone needs an earlier winframe that found the panel", "a window number", None)
    return all(w["number"] != wid for w in windows)


def run_winframe(ctx: RunContext, step: dict) -> str:
    target = step["winframe"]

    if target == "gone":
        # Read from Quartz's on-screen window list, not from the app: the app saying
        # it hid the panel is `state.shell.visible`; this checks the window server.
        def gone():
            wid = ctx.last_panel_wid
            if not window_gone(wid, screen_windows(ctx.inst.proc.pid)):
                raise StepFailed(f"window {wid} still on screen", "absent from CGWindowList", "on screen")
            return f"window {wid} no longer on screen"

        detail = _poll(step.get("timeout_ms", 2000), gone)
        # Used up: a second `gone` must first see a window again.
        ctx.last_panel_wid = None
        return detail

    def attempt():
        wid, bounds = _panel_window(ctx)
        displays = active_displays()
        if target == "any":
            if any(rect_within(bounds, d["bounds"]) for d in displays):
                return f"window {wid} {bounds} lies within a display"
            raise StepFailed("window off-screen", [d["bounds"] for d in displays], bounds)
        d = _display_for(target, displays, ctx)
        if not rect_within(bounds, d["bounds"]):
            raise StepFailed(f"window not within display {d['id']}", d["bounds"], bounds)
        return f"window {wid} {bounds} lies within display {d['id']}"

    return _poll(step.get("timeout_ms", 2000), attempt)


def run_warp_cursor(ctx: RunContext, step: dict) -> str:
    v = step["warp_cursor"]
    spec = v if isinstance(v, dict) else {"display": v}
    d = _display_for(spec["display"], active_displays(), ctx)
    b = d["bounds"]
    x = b["x"] + b["w"] * spec.get("x", 0.5)
    y = b["y"] + b["h"] * spec.get("y", 0.5)
    now = quartz("warp", x, y)
    return f"cursor at ({now['x']:.0f}, {now['y']:.0f}) on display {d['id']}"


# Every activator process this probe started, so the run's teardown can stop any
# that a step did not (Ctrl-C, SIGTERM mid-step).
_ACTIVATORS: list = []
_ACTIVATOR_TIMEOUT_S = 8.0


def stop_activators():
    for proc in _ACTIVATORS:
        if proc.poll() is None:
            with contextlib.suppress(OSError):
                proc.stdin.close()
            try:
                proc.wait(timeout=2)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait()
    _ACTIVATORS.clear()


def run_activate_other(ctx: RunContext, step: dict) -> str:
    # A real loss of key status, the way the user clicking into another app causes
    # it — not the resign_key verb's shortcut. The "other app" is the probe's own
    # activator (scripts/probe-helpers/activator.swift), never one of the user's
    # apps (O4), and it is gone again when this step ends.
    import select

    proc = subprocess.Popen(
        [str(_helper(_ACTIVATOR_SWIFT))], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True
    )
    _ACTIVATORS.append(proc)
    ctx.own_pids.add(proc.pid)
    try:
        ready, _, _ = select.select([proc.stdout], [], [], _ACTIVATOR_TIMEOUT_S)
        line = proc.stdout.readline().strip() if ready else ""
        if not line.startswith("active"):
            raise EnvBlocked(f"the probe's activator could not take focus ({line or 'no answer'})")
        time.sleep(step.get("settle_ms", 300) / 1000.0)
    finally:
        stop_activators()
        # Let AppKit hand activation back before the next step reads the frontmost
        # app, so the focus-theft check never mistakes that hand-back for a theft.
        deadline = time.monotonic() + 1.5
        while time.monotonic() < deadline and quartz("frontmost").get("pid") == proc.pid:
            time.sleep(0.05)
    return f"probe activator (pid {proc.pid}) took key status, then quit"


def run_restart(ctx: RunContext, step: dict) -> str:
    ctx.inst.restart()
    return f"restarted (pid {ctx.inst.proc.pid}, {len(ctx.inst._fixtures)} fixture(s) replayed)"


def run_sleep(ctx: RunContext, step: dict) -> str:
    time.sleep(step["sleep_ms"] / 1000.0)
    return f"slept {step['sleep_ms']} ms"


STEP_RUNNERS = {
    "call": run_call,
    "see": run_see,
    "ocr": run_ocr,
    "pixel": run_pixel,
    "pasteboard": run_pasteboard,
    "graph": run_graph,
    "file": run_file,
    "winframe": run_winframe,
    "warp_cursor": run_warp_cursor,
    "activate_other": run_activate_other,
    "restart": run_restart,
    "sleep_ms": run_sleep,
}
assert set(STEP_RUNNERS) == set(STEP_TYPES)


# ── the run command ───────────────────────────────────────────────────────


class Tally:
    def __init__(self):
        self.passed = self.failed = self.blocked = self.errors = 0

    def exit_code(self) -> int:
        # 2 = a scenario is wrong, 1 = the product (or the harness) failed,
        # 3 = only the environment blocked steps; 0 only if everything passed.
        if self.errors:
            return 2
        if self.failed:
            return 1
        if self.blocked:
            return 3
        return 0


def _describe(kind: str, step: dict) -> str:
    v = step[kind]
    if kind == "call" and step.get("args"):
        v = f"{v} {_short(step['args'], 80)}"
    elif not isinstance(v, str):
        v = _short(v, 80)
    label = f" ({step['name']})" if step.get("name") else ""
    return f"{kind} {v}{label}"


def _report_fail(e: StepFailed):
    print(f"      {e}")
    if e.expected is not None or e.actual is not None:
        print(f"      expected: {_short(e.expected, 600)}")
        print(f"      actual:   {_short(e.actual, 600)}")


_VISIBILITY_PATHS = re.compile(r"(^|\.)(visible|route|closed|window_id)$")


def is_visibility_step(kind: str, step: dict) -> bool:
    """Steps whose failure can be caused by another app taking focus: they assert the
    panel is shown or hidden (a capture/browser panel hides on any loss of key
    status, by design)."""
    if kind == "winframe":
        return True
    if kind != "call":
        return False
    if step["call"] in ("window_id", "resign_key"):
        return True
    return any(_VISIBILITY_PATHS.search(p) for p in (step.get("expect") or {}))


def focus_thief(before, after, own_pids: set):
    """The name of an app that became frontmost during a step, or None. The app that
    was frontmost when the step began (the user's terminal) and the probe's own
    processes (itself, the instance, its activator) never count."""
    if not isinstance(after, dict) or not after.get("pid"):
        return None
    if after["pid"] in own_pids:
        return None
    if isinstance(before, dict) and before.get("pid") == after["pid"]:
        return None
    return after.get("name") or f"pid {after['pid']}"


def step_units(steps: list) -> list:
    """Group steps into iterations: consecutive steps sharing a `block` id form one,
    every other step is its own. Returns lists of (1-based index, step)."""
    units, current, current_block = [], [], None
    for i, step in enumerate(steps, 1):
        block = step.get("block")
        if current and (block is None or block != current_block):
            units.append(current)
            current = []
        current.append((i, step))
        current_block = block
        if block is None:
            units.append(current)
            current, current_block = [], None
    if current:
        units.append(current)
    return units


def _frontmost():
    try:
        return quartz("frontmost")
    except ProbeError:
        return None


def reset_panel(ctx: RunContext):
    """Before re-running an iteration: no panel open. `open_view capture` then
    `resign_key` closes whatever is showing (capture hides on resign_key)."""
    if visible_in(ctx.inst.call("state")):
        ctx.inst.call("open_view", route="capture")
        ctx.inst.call("resign_key")
    ctx.panel_visible = False
    ctx.last_panel_wid = None


def run_steps(ctx: RunContext, tally: Tally, prefix: str) -> bool:
    """Run every step against one instance; stop at the first FAIL. Returns ok.

    Unattended guard (Phase 1 R15 runs these with nobody watching): when a step that
    asserts the panel's visibility fails and another app became frontmost during
    it, the failure may be that app's doing, not the product's. The iteration (its
    `block`, or the step alone) is re-run once from a closed panel; a second theft
    makes it ENV-BLOCKED ("focus taken by <app>"), never a FAIL."""
    steps = ctx.scenario["steps"]
    for unit in step_units(steps):
        for attempt in (1, 2):
            outcome, passed = _run_unit(ctx, unit, tally, prefix, len(steps), retry_left=attempt == 1)
            if outcome == "retry":
                tally.passed -= passed
                try:
                    reset_panel(ctx)
                except (ProbeError, CallError) as e:
                    tally.failed += 1
                    print(f"FAIL  {prefix}: could not reset the panel for a retry: {e}")
                    return False
                continue
            if outcome == "fail":
                return False
            break
    return True


def _run_unit(ctx, unit, tally, prefix, total, retry_left):
    """One attempt at an iteration. Returns ("ok"|"retry"|"blocked"|"fail", passes)."""
    passed = 0
    for n, (i, step) in enumerate(unit):
        kind = step_type(step, f"step {i}")
        head = f"{prefix} [{i}/{total}] {_describe(kind, step)}"
        before = _frontmost() if is_visibility_step(kind, step) else None
        try:
            detail = STEP_RUNNERS[kind](ctx, step)
        except EnvBlocked as e:
            tally.blocked += 1
            print(f"ENV-BLOCKED {head}\n      {e}")
            continue
        except ScenarioError as e:
            tally.errors += 1
            print(f"ERROR {head}\n      {e}")
            return "fail", passed
        except (StepFailed, ProbeError, CallError) as e:
            thief = None
            if isinstance(e, StepFailed) and is_visibility_step(kind, step):
                thief = focus_thief(before, _frontmost(), ctx.owned_pids())
            if thief and retry_left:
                print(f"RETRY {head}\n      focus taken by {thief} during the step: {e}\n"
                      f"      re-running this iteration once")
                return "retry", passed
            if thief:
                tally.blocked += 1
                skipped = len(unit) - n - 1
                print(f"ENV-BLOCKED {head}\n      focus taken by {thief} (twice): {e}"
                      + (f"\n      ({skipped} later step(s) of this iteration skipped)" if skipped else ""))
                return "blocked", passed
            tally.failed += 1
            print(f"FAIL  {head}")
            if isinstance(e, StepFailed):
                _report_fail(e)
            else:
                print(f"      {e}")
            if i < total:
                print(f"      (stopping: {total - i} later step(s) not run)")
            return "fail", passed
        tally.passed += 1
        passed += 1
        print(f"PASS  {head} — {detail}")
    return "ok", passed


def _check(tally: Tally, head: str, fn):
    try:
        detail = fn()
    except (StepFailed, ProbeError, CallError) as e:
        tally.failed += 1
        print(f"FAIL  {head}")
        if isinstance(e, StepFailed):
            _report_fail(e)
        else:
            print(f"      {e}")
        return
    tally.passed += 1
    print(f"PASS  {head} — {detail}")


def side_by_side_checks(name: str, instances: list, tally: Tally):
    """R8/O1 with --instances N: every instance still answers while all run, and none
    shares a socket, data dir or fixture with another."""
    for k, inst in enumerate(instances):
        prefix = f"{name}#{k + 1}"

        def answers_state(inst=inst):
            st = inst.call("state")
            if not isinstance(st, dict) or "shell" not in st:
                raise StepFailed("state shape", "an object with shell", st)
            # Isolation, observed from inside the app: the data dir its one resolver
            # (paths::data_dir) returns is the one this probe gave it, not a
            # sibling's and not the user's.
            gd = st.get("data_dir")
            if gd != str(inst.data_dir):
                raise StepFailed("instance data dir", str(inst.data_dir), gd)
            return f"pid {inst.proc.pid}, socket {inst.socket_path}, data dir {gd}"

        def opened_is_sink(inst=inst):
            got = inst.call("opened")
            if not isinstance(got, list):
                raise StepFailed("opened sink", "a list of recorded opens", got)
            return f"sink answers ({len(got)} recorded open(s), nothing launched)"

        _check(tally, f"{prefix} [side-by-side] answers state while {len(instances)} run", answers_state)
        _check(tally, f"{prefix} [side-by-side] opened reports the sink", opened_is_sink)

    def distinct():
        for attr in ("socket_path", "data_dir", "fixtures_dir", "root"):
            vals = [str(getattr(i, attr)) for i in instances]
            if len(set(vals)) != len(vals):
                raise StepFailed(f"shared {attr}", "distinct per instance", vals)
        pids = [i.proc.pid for i in instances]
        if len(set(pids)) != len(pids) or any(i.proc.poll() is not None for i in instances):
            raise StepFailed("processes", "distinct live pids", pids)
        return f"{len(instances)} distinct pids, sockets, data dirs and fixture dirs"

    _check(tally, f"{name} [side-by-side] isolation", distinct)


def _copy_setup_files(inst: Instance, setup: dict):
    tokens = {"root": inst.root, "data_dir": inst.data_dir, "fixtures": inst.fixtures_dir,
              "probes": PROBES_DIR, "instance": inst.name}
    for dest, src in (setup.get("files") or {}).items():
        d = Path(expand(dest, tokens))
        s = Path(expand(src, tokens))
        if not s.is_absolute():
            s = inst.fixtures_dir / s
        if not s.exists():
            raise ScenarioError(f"setup.files: {s} not found")
        d.parent.mkdir(parents=True, exist_ok=True)
        if s.is_dir():
            shutil.copytree(s, d, dirs_exist_ok=True)
        else:
            shutil.copy2(s, d)
    return tokens


def run_scenario(sc: dict, binary: Path, n: int, tally: Tally):
    name = sc["_name"]
    print(f"── {name}: {sc.get('description', '')}".rstrip(": "))
    instances = [Instance(f"{name}-{k + 1}" if n > 1 else name, binary) for k in range(n)]
    try:
        setup = sc.get("setup", {})
        for inst in instances:
            inst.prepare()
            tokens = _copy_setup_files(inst, setup)
            inst.start()
            for kind, path in (setup.get("fixtures") or {}).items():
                p = Path(expand(path, tokens))
                if not p.is_absolute():
                    p = inst.fixtures_dir / p
                inst.call("load_fixture", kind=kind, path=str(p))
        for k, inst in enumerate(instances):
            prefix = f"{name}#{k + 1}" if n > 1 else name
            run_steps(RunContext(inst, sc), tally, prefix)
        if n > 1:
            side_by_side_checks(name, instances, tally)
    except ScenarioError as e:
        tally.errors += 1
        print(f"ERROR {name}: {e}")
    except (ProbeError, CallError) as e:
        tally.failed += 1
        print(f"FAIL  {name}: {e}")
    finally:
        for inst in instances:
            inst.stop()


def keep_awake():
    """Hold display and idle-sleep assertions for the whole run (an unattended run
    must not watch the display sleep under it): `caffeinate -d -i -u`, tied to this
    process with `-w` so it ends even if the probe is killed, and terminated in
    `cmd_run`'s teardown. `-u` also wakes a display that is already asleep."""
    try:
        return subprocess.Popen(
            ["caffeinate", "-d", "-i", "-u", "-t", "86400", "-w", str(os.getpid())],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
    except FileNotFoundError:
        return None


def wait_for_display(timeout_s: float = 5.0):
    """Give a display woken by `keep_awake` a moment to come back. Not an error if it
    does not: the steps that need one report "display asleep" as ENV-BLOCKED."""
    deadline = time.monotonic() + timeout_s
    while time.monotonic() < deadline:
        with contextlib.suppress(ProbeError):
            if quartz("displays"):
                return
        time.sleep(0.25)


def cmd_run(args: argparse.Namespace) -> int:
    """Run scenario files against fresh isolated automation instances (R8)."""
    tally = Tally()
    try:
        scenarios = [load_scenario(n) for n in args.names]
    except ScenarioError as e:
        print(f"ERROR {e}")
        return 2
    if args.instances < 1:
        print("ERROR --instances must be >= 1")
        return 2

    # A SIGTERM from a verifier's timeout must still stop the instances and remove
    # their temp roots: turn it into SystemExit so the finally blocks run.
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))
    # This run's own copy of the binary lives here until the run ends (build_binary).
    run_dir = Path(tempfile.mkdtemp(prefix="ubprobe-run-", dir=_tmpdir()))
    build = None
    awake = keep_awake()
    try:
        build = build_binary(run_dir)
        binary = build
        # Compile (and, for OCR, warm) helpers up front, so neither the swiftc log nor
        # Vision's one-off model compile lands inside a step.
        _helper(_QUARTZ_SWIFT)
        if any(step_type(st, "") == "ocr" for sc in scenarios for st in sc["steps"]):
            _helper(_OCR_SWIFT)
        if any(step_type(st, "") == "activate_other" for sc in scenarios for st in sc["steps"]):
            _helper(_ACTIVATOR_SWIFT)
        wait_for_display()
        with desktop_lock():
            for sc in scenarios:
                run_scenario(sc, binary, args.instances, tally)
    except ProbeError as e:
        print(f"FAIL  {e}")
        return 1
    except EnvBlocked as e:
        print(f"ENV-BLOCKED {e}")
        return 3
    finally:
        stop_activators()
        restore_pasteboard()
        shutil.rmtree(run_dir, ignore_errors=True)
        if awake is not None:
            awake.terminate()
            with contextlib.suppress(subprocess.TimeoutExpired):
                awake.wait(timeout=5)

    code = tally.exit_code()
    verdict = {0: "PASS", 1: "FAIL", 2: "ERROR", 3: "ENV-BLOCKED"}[code]
    print(
        f"{verdict}: run {' '.join(args.names)} (instances={args.instances}) — "
        f"{tally.passed} passed, {tally.failed} failed, {tally.blocked} env-blocked, "
        f"{tally.errors} scenario error(s)"
    )
    # Evidence names what ran: the exact binary and the source it came from.
    print(f"ran: {build.describe() if build else 'no binary'}")
    return code


# ══════════════════════════════════════════════════════════════════════════
# `doctor`: read-only unattended-run preflight (R9, O4, TN5; Phase 1 D5)
# ══════════════════════════════════════════════════════════════════════════
#
# Phase 1 runs `run` unattended under factory-conductor for hours; if the host
# cannot do GUI probes (Screen Recording revoked, display asleep, no Swift
# toolchain) that run should refuse to start instead of grinding through
# ENV-BLOCKED or false FAILs with nobody watching. `doctor` is that gate (the
# verify skill's Doctor section, task 14, calls it before a real run), and it
# is read-only: no check here may grant a permission, flip a setting, or leave
# a helper/caffeinate process running past its own check.

_RUST_TOOLCHAIN_TOML = REPO_ROOT / "rust-toolchain.toml"
_TOOLCHAIN_CHANNEL_RE = re.compile(r'^\s*channel\s*=\s*"([0-9][\w.\-]*)"\s*$')
# How long the caffeinate check's throwaway assertion runs; short because doctor
# itself must stay fast, long enough that `pmset -g assertions` reliably sees it.
_CAFFEINATE_PROBE_S = 2.0


def _process_name(pid: int) -> str:
    """The short command name macOS shows for `pid` (e.g. "python3", "Terminal")."""
    proc = subprocess.run(["ps", "-p", str(pid), "-o", "comm="], capture_output=True, text=True)
    name = proc.stdout.strip()
    return os.path.basename(name) if name else f"pid {pid}"


def _parent_pid(pid: int):
    proc = subprocess.run(["ps", "-p", str(pid), "-o", "ppid="], capture_output=True, text=True)
    try:
        return int(proc.stdout.strip())
    except ValueError:
        return None


def _process_chain(pid: int, depth: int = 6) -> list:
    """[name at pid, name at its parent, ...], up to `depth` entries.

    TCC grants Screen Recording to the *process* that calls the CGWindow API, not
    to python3 in the abstract: when probe.py runs inside a Terminal tab, an IDE's
    integrated shell, or a CI agent's own process, that host app is what System
    Settings needs the grant for. `check_screen_recording` reports this chain as a
    hint, never as a fix it applies itself (granting it is a human/System Settings
    action, which a read-only doctor must not attempt).
    """
    chain = []
    seen = set()
    cur = pid
    for _ in range(depth):
        if cur is None or cur in seen or cur <= 1:
            break
        seen.add(cur)
        chain.append(_process_name(cur))
        cur = _parent_pid(cur)
    return chain


def check_screen_recording() -> dict:
    """CGPreflightScreenCaptureAccess via quartz.swift; never prompts (TN5)."""
    try:
        granted = bool(quartz("screen-access").get("granted"))
    except (ProbeError, EnvBlocked) as e:
        return {
            "name": "screen_recording", "status": "FAIL",
            "detail": f"could not query CGPreflightScreenCaptureAccess: {e}",
        }
    if granted:
        return {"name": "screen_recording", "status": "PASS", "detail": "granted to this process"}
    chain = " -> ".join(_process_chain(os.getpid())) or f"pid {os.getpid()}"
    return {
        "name": "screen_recording", "status": "FAIL",
        "detail": (
            "denied - grant Screen Recording (System Settings > Privacy & Security > "
            f"Screen Recording) to the host app in this process chain: {chain}"
        ),
    }


def check_display_awake() -> dict:
    """CGDisplayIsAsleep on the main display."""
    try:
        info = quartz("display-asleep")
    except (ProbeError, EnvBlocked) as e:
        return {"name": "display_awake", "status": "FAIL", "detail": f"could not query CGDisplayIsAsleep: {e}"}
    if info.get("asleep"):
        return {"name": "display_awake", "status": "FAIL", "detail": f"main display {info.get('id')} is asleep"}
    return {"name": "display_awake", "status": "PASS", "detail": f"main display {info.get('id')} is awake"}


def check_display_count() -> dict:
    """Active display count via CGGetActiveDisplayList; >=1 required.

    N=1 is a PASS, not a WARN: single-display scenarios run fine with one
    display. It only means a *multi*-display scenario will be ENV-BLOCKED on
    this host (`_display_for` in the `run` engine), which this check reports as
    an informational note, not a reason to fail doctor.
    """
    try:
        displays = quartz("displays")
    except (ProbeError, EnvBlocked) as e:
        return {
            "name": "display_count", "status": "FAIL",
            "detail": f"could not query CGGetActiveDisplayList: {e}",
        }
    n = len(displays)
    if n < 1:
        return {
            "name": "display_count", "status": "FAIL",
            "detail": "0 active displays (display asleep or no window server session)",
        }
    detail = f"{n} active display(s)"
    if n == 1:
        detail += " - a multi-display scenario will be ENV-BLOCKED on this host (informational)"
    return {"name": "display_count", "status": "PASS", "detail": detail}


def check_ocr_helper() -> dict:
    """Compile scripts/ocr.swift if needed, then run it on a generated PNG.

    The generated blank PNG (not a screencapture of the real screen) means this
    check never depends on the Screen Recording grant `check_screen_recording`
    reports separately - it proves the Vision pipeline itself compiles and runs,
    not that it reads any particular text.
    """
    try:
        binary = _helper(_OCR_SWIFT)
    except (ProbeError, EnvBlocked) as e:
        return {"name": "ocr_helper", "status": "FAIL", "detail": f"compile failed: {e}"}
    png = binary.with_name(f"doctor-{os.getpid()}.png")
    try:
        _tiny_png(png)
        out = _run_helper(_OCR_SWIFT, png, timeout=_OCR_TIMEOUT_S, attempts=2)
    except (ProbeError, EnvBlocked, OSError) as e:
        return {
            "name": "ocr_helper", "status": "FAIL",
            "detail": f"compiled at {binary.name} but failed to run: {e}",
        }
    finally:
        png.unlink(missing_ok=True)
    if not isinstance(out, dict) or "lines" not in out:
        return {"name": "ocr_helper", "status": "FAIL", "detail": f"unexpected output: {out!r}"}
    return {
        "name": "ocr_helper", "status": "PASS",
        "detail": f"compiled at {binary.name}, ran on a generated PNG ({len(out['lines'])} line(s) read)",
    }


def _start_caffeinate(seconds: float):
    """Start a throwaway `caffeinate -d -u -t <seconds>`; None if not on PATH."""
    if not shutil.which("caffeinate"):
        return None
    try:
        return subprocess.Popen(
            ["caffeinate", "-d", "-u", "-t", str(seconds)],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
    except OSError:
        return None


def _pmset_assertions() -> str:
    try:
        proc = subprocess.run(["pmset", "-g", "assertions"], capture_output=True, text=True)
    except FileNotFoundError:
        return ""
    return proc.stdout if proc.returncode == 0 else ""


def _pmset_has_assertion(text: str, pid: int, name: str) -> bool:
    """Whether `pmset -g assertions`'s text lists assertion `name` owned by `pid`."""
    marker = f"pid {pid}("
    return any(marker in line and name in line for line in text.splitlines())


def check_caffeinate() -> dict:
    """Prove the run-time keep_awake() guard will work, without leaving anything running.

    `cmd_run`'s `keep_awake()` holds a caffeinate assertion for the whole
    unattended run (R9); doctor itself is short, so instead of trusting that
    caffeinate exists, it starts a throwaway assertion and reads it back from
    `pmset -g assertions` - the same ground truth an operator would check by
    hand - then kills it rather than waiting out the full duration.
    """
    proc = _start_caffeinate(_CAFFEINATE_PROBE_S)
    if proc is None:
        return {
            "name": "caffeinate", "status": "FAIL",
            "detail": "caffeinate not found on PATH (or failed to start)",
        }
    try:
        deadline = time.monotonic() + _CAFFEINATE_PROBE_S
        while time.monotonic() < deadline:
            if _pmset_has_assertion(_pmset_assertions(), proc.pid, "PreventUserIdleDisplaySleep"):
                return {
                    "name": "caffeinate", "status": "PASS",
                    "detail": f"pid {proc.pid} holds a PreventUserIdleDisplaySleep assertion",
                }
            time.sleep(0.1)
        return {
            "name": "caffeinate", "status": "FAIL",
            "detail": (
                f"no PreventUserIdleDisplaySleep assertion from caffeinate pid {proc.pid} "
                "appeared in pmset -g assertions"
            ),
        }
    finally:
        if proc.poll() is None:
            proc.terminate()
        with contextlib.suppress(subprocess.TimeoutExpired):
            proc.wait(timeout=3)
        if proc.poll() is None:
            proc.kill()
            proc.wait(timeout=3)


def _swiftc_version() -> str:
    try:
        proc = subprocess.run(["swiftc", "--version"], capture_output=True, text=True)
    except FileNotFoundError:
        return ""
    text = (proc.stdout or proc.stderr).strip()
    return text if proc.returncode == 0 else ""


def check_xcode_swiftc() -> dict:
    """Xcode command line tools: swiftc is what compiles every helper in this file."""
    version = _swiftc_version()
    if not version:
        return {
            "name": "xcode_swiftc", "status": "FAIL",
            "detail": "swiftc not found (install the Xcode command line tools)",
        }
    return {"name": "xcode_swiftc", "status": "PASS", "detail": version.splitlines()[0]}


def _rust_toolchain_channel(path: Path = None) -> str:
    """The `[toolchain] channel` rust-toolchain.toml pins (R1's single source of
    truth); raises ValueError if the file has no channel line."""
    path = path or _RUST_TOOLCHAIN_TOML
    text = path.read_text(encoding="utf-8")
    for line in text.splitlines():
        m = _TOOLCHAIN_CHANNEL_RE.match(line)
        if m:
            return m.group(1)
    raise ValueError(f"no [toolchain] channel found in {path}")


def _rustc_version() -> str:
    try:
        proc = subprocess.run(["rustc", "--version"], cwd=REPO_ROOT, capture_output=True, text=True)
    except FileNotFoundError:
        return ""
    return proc.stdout.strip() if proc.returncode == 0 else ""


def check_rust_toolchain() -> dict:
    """rustc --version against rust-toolchain.toml's channel.

    Reuses only `cmd_toolchain`'s rustc check, not its workflow-file check: that
    one fails until a pending patch to .github/workflows lands (see task 9's
    brief), and doctor's job is "can this host build/run", not "is CI in sync".
    """
    try:
        expect = _rust_toolchain_channel()
    except (OSError, ValueError) as e:
        return {
            "name": "rust_toolchain", "status": "FAIL",
            "detail": f"could not read rust-toolchain.toml: {e}",
        }
    rustc_version = _rustc_version()
    if not rustc_version:
        return {"name": "rust_toolchain", "status": "FAIL", "detail": "rustc not found on PATH"}
    expected_prefix = f"rustc {expect}"
    if not rustc_version.startswith(expected_prefix):
        return {
            "name": "rust_toolchain", "status": "FAIL",
            "detail": f"rustc --version printed {rustc_version!r}, expected it to start with {expected_prefix!r}",
        }
    return {"name": "rust_toolchain", "status": "PASS", "detail": rustc_version}


def check_automation_build(build: bool) -> dict:
    """The locked `--features automation` build `run` needs; slow (a full cargo
    build), so only runs with --build - doctor stays fast by default."""
    if not build:
        return {
            "name": "automation_build", "status": "WARN",
            "detail": "skipped (pass --build to run the locked automation build)",
        }
    run_dir = Path(tempfile.mkdtemp(prefix="ubprobe-doctor-", dir=_tmpdir()))
    try:
        built = build_binary(run_dir)
    except ProbeError as e:
        return {"name": "automation_build", "status": "FAIL", "detail": f"build failed: {e}"}
    finally:
        shutil.rmtree(run_dir, ignore_errors=True)
    return {"name": "automation_build", "status": "PASS", "detail": built.describe()}


# Every check but automation_build (which needs the --build flag threaded through);
# order here is the order doctor prints them in.
_DOCTOR_CHECKS = (
    check_screen_recording,
    check_display_awake,
    check_display_count,
    check_xcode_swiftc,
    check_rust_toolchain,
    check_ocr_helper,
    check_caffeinate,
)


def _run_doctor_checks(build: bool = False) -> list:
    """Run every doctor check, never letting one crash stop the rest."""
    checks = []
    for fn in _DOCTOR_CHECKS:
        try:
            checks.append(fn())
        except Exception as e:  # a check must never take doctor itself down
            checks.append({
                "name": fn.__name__[len("check_"):], "status": "FAIL",
                "detail": f"check raised {e!r}",
            })
    try:
        checks.append(check_automation_build(build))
    except Exception as e:
        checks.append({"name": "automation_build", "status": "FAIL", "detail": f"check raised {e!r}"})
    return checks


def doctor_status(checks: list) -> str:
    """FAIL if any check FAILed; a WARN (the skipped --build check, or an
    informational N=1 display note folded into a PASS) never fails doctor alone."""
    return "FAIL" if any(c["status"] == "FAIL" for c in checks) else "PASS"


def cmd_doctor(args: argparse.Namespace) -> int:
    """Read-only preflight: can this Mac host GUI probes unattended? (R9, O4,
    TN5; Phase 1 D5). See this section's banner comment above for why."""
    checks = _run_doctor_checks(build=args.build)
    status = doctor_status(checks)
    if args.json:
        print(json.dumps({"result": status, "checks": checks}, indent=2))
    else:
        for c in checks:
            print(f"{c['status']} {c['name']}: {c['detail']}")
        print(f"DOCTOR_RESULT: {status}")
    return 0 if status == "PASS" else 1


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="probe.py",
        description="GPUI probe harness: scriptable checks for the GPUI port plan.",
    )
    subparsers = parser.add_subparsers(dest="command", required=True)

    # ── toolchain ──────────────────────────────────────────────────────
    p_toolchain = subparsers.add_parser(
        "toolchain",
        help="Check rustc --version and every workflow's toolchain input match --expect.",
    )
    p_toolchain.add_argument(
        "--expect",
        required=True,
        help='Version prefix to require, e.g. "1.98" (matches rustc 1.98.1, etc).',
    )
    p_toolchain.add_argument(
        "--workflows-dir",
        default=None,
        help="Override .github/workflows (e.g. a scratch copy, for testing a pending patch).",
    )
    p_toolchain.set_defaults(func=cmd_toolchain)

    # ── deps-clean ─────────────────────────────────────────────────────
    p_deps_clean = subparsers.add_parser(
        "deps-clean",
        help="Fail if <manifest>'s dependency tree pulls in a tauri*/gpui*/wry/tao/webkit* crate.",
    )
    p_deps_clean.add_argument(
        "manifest",
        help="Path to the Cargo.toml to check, e.g. app/Cargo.toml.",
    )
    p_deps_clean.set_defaults(func=cmd_deps_clean)

    # ── release-clean ──────────────────────────────────────────────────
    p_release_clean = subparsers.add_parser(
        "release-clean",
        help="Fail if the desktop-gpui release binary contains the automation socket, marker or verbs.",
    )
    p_release_clean.add_argument(
        "--skip-bite-check",
        action="store_true",
        help="Skip the --features automation build that proves the check can find the marker.",
    )
    p_release_clean.set_defaults(func=cmd_release_clean)

    # ── run ────────────────────────────────────────────────────────────
    p_run = subparsers.add_parser(
        "run",
        help="Run desktop-gpui/probes/<name>.json scenarios against isolated automation instances.",
    )
    p_run.add_argument("names", nargs="+", help="Scenario names (desktop-gpui/probes/<name>.json) or paths.")
    p_run.add_argument(
        "--instances",
        type=int,
        default=1,
        help="Run each scenario against N simultaneous instances and check they stay isolated.",
    )
    p_run.set_defaults(func=cmd_run)

    # ── fixtures ───────────────────────────────────────────────────────
    p_fixtures = subparsers.add_parser(
        "fixtures",
        help="Validate or (re)generate desktop-gpui/probes/fixtures/*.json (Task 10, Phase 1 R1-R11).",
    )
    fx_group = p_fixtures.add_mutually_exclusive_group(required=True)
    fx_group.add_argument(
        "--check", action="store_true",
        help="Validate every fixture against its documented schema (desktop-gpui/probes/fixtures/README.md).",
    )
    fx_group.add_argument(
        "--generate", action="store_true",
        help="Regenerate graph-5000.json deterministically from its seed.",
    )
    p_fixtures.set_defaults(func=cmd_fixtures)

    # ── doctor ─────────────────────────────────────────────────────────
    p_doctor = subparsers.add_parser(
        "doctor",
        help="Read-only preflight: can this Mac host GUI probes unattended? (R9)",
    )
    p_doctor.add_argument(
        "--json", action="store_true", help="Machine-readable output instead of PASS/FAIL lines."
    )
    p_doctor.add_argument(
        "--build", action="store_true",
        help="Also run the locked --features automation build (slow; off by default).",
    )
    p_doctor.set_defaults(func=cmd_doctor)

    return parser


def main(argv=None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
