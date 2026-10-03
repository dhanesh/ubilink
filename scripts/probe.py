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
"""

import argparse
import re
import subprocess
import sys
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

    return parser


def main(argv=None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
