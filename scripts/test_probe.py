"""Self-tests for scripts/probe.py's scenario engine (GPUI Phase 0 R8).

Headless and stdlib-only: everything here runs without building or launching the
app, so it covers the parts a live `probe.py run` cannot prove on its own — that a
typo in a scenario is refused instead of passing vacuously, that matching tolerates
truncation without matching everything, that ENV-BLOCKED is not reported as a
product FAIL, and that the desktop lock really excludes a second holder.

    python3 -m unittest scripts/test_probe.py
"""

import argparse
import contextlib
import io
import json
import os
import re
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import textwrap
import unittest
from pathlib import Path

# No scripts/__pycache__ left behind in the repo by importing probe.py.
sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
import probe  # noqa: E402


class PathAndMatchers(unittest.TestCase):
    doc = {"shell": {"window_kind": "popup", "hotkeys_registered": []}, "items": [{"uri": "a"}, {"uri": "b"}]}

    def test_get_path(self):
        self.assertEqual(probe.get_path(self.doc, "shell.window_kind"), "popup")
        self.assertEqual(probe.get_path(self.doc, "items.1.uri"), "b")
        self.assertEqual(probe.get_path(self.doc, "items.-1.uri"), "b")
        self.assertIs(probe.get_path(self.doc, "items.9.uri"), probe._MISSING)
        self.assertIs(probe.get_path(self.doc, "shell.nope"), probe._MISSING)
        self.assertIs(probe.get_path(self.doc, "shell.window_kind.x"), probe._MISSING)
        self.assertEqual(probe.get_path(self.doc, ""), self.doc)

    def test_literals_compare_for_equality(self):
        self.assertEqual(probe.check_expect(self.doc, {"shell.window_kind": "popup"}), [])
        misses = probe.check_expect(self.doc, {"shell.window_kind": "other", "x": 1})
        self.assertEqual([m[0] for m in misses], ["shell.window_kind", "x"])
        self.assertEqual(misses[1][2], "<missing>")
        # A missing key never equals a literal, not even null.
        self.assertTrue(probe.check_expect(self.doc, {"route": None}))
        self.assertEqual(probe.check_expect({"route": None}, {"route": None}), [])

    def test_operators(self):
        ok = lambda a, spec: probe.match_value(a, spec)  # noqa: E731
        self.assertTrue(ok([], {"len": 0}))
        self.assertTrue(ok([1, 2], {"min_len": 2, "max_len": 2}))
        self.assertTrue(ok(5, {"gt": 0, "lt": 10}))
        self.assertFalse(ok(True, {"gt": 0}), "bools are not numbers")
        self.assertTrue(ok(3, {"type": "number"}))
        self.assertFalse(ok(True, {"type": "number"}))
        self.assertTrue(ok("abc/export.json", {"matches": r"export\.json$"}))
        self.assertTrue(ok(["a", "b"], {"contains": "a"}))
        self.assertTrue(ok("popup", {"in": ["popup", "other"]}))
        self.assertTrue(ok("Capture: https://exa…", {"text": "https://example.com"}))
        self.assertTrue(ok(probe._MISSING, {"exists": False}))
        self.assertFalse(ok(probe._MISSING, {"type": "null"}))
        # An object with a non-operator key is a literal, compared for equality.
        self.assertTrue(ok({"len": 1, "x": 2}, {"len": 1, "x": 2}))
        self.assertFalse(ok([1], {"len": 1, "x": 2}))


class TextMatching(unittest.TestCase):
    def test_contains_normalised(self):
        self.assertTrue(probe.text_matches("capture", "Capture"))
        self.assertTrue(probe.text_matches("browse  links", "Browse links"))
        self.assertTrue(probe.text_matches("ＡＢＣ", "abc"), "NFKC folds full-width forms")
        self.assertFalse(probe.text_matches("Settings", "Capture"))

    def test_exact(self):
        self.assertTrue(probe.text_matches("Capture", "capture", exact=True))
        self.assertFalse(probe.text_matches("Capture", "Capture link", exact=True))

    def test_trailing_ellipsis(self):
        have = "Capture: https://exa…"
        self.assertTrue(probe.text_matches("https://example.com/page", have))
        self.assertTrue(probe.text_matches("https://example.com/page", "https://example.co...", exact=True))
        # Too little visible before the ellipsis to be evidence.
        self.assertFalse(probe.text_matches("https://example.com", "x: htt…"))
        # The visible head must agree with what we want.
        self.assertFalse(probe.text_matches("https://other.org", have))

    def test_leading_ellipsis(self):
        self.assertTrue(probe.text_matches("/Users/me/notes/plan.md", "…notes/plan.md"))
        self.assertFalse(probe.text_matches("/Users/me/notes/plan.md", "…notes/other.md"))


class A11y(unittest.TestCase):
    # The shape Task 6 recorded live from debug_a11y_tree_json.
    tree = {
        "frame": {"frame_number": 3},
        "nodes": {
            "a": {"aria": {"role": "Label", "value": "Capture"}},
            "b": {"accesskit_id": "0", "children": ["a"], "aria": {"role": "Window"}},
            "c": {"aria": {"role": "TextInput", "placeholder": "Filter links…", "label": "Filter"}},
        },
    }

    def test_find_by_role_and_text(self):
        self.assertEqual(len(probe.find_nodes(self.tree, "Label", "capture")), 1)
        self.assertEqual(len(probe.find_nodes(self.tree, "label")), 1, "role is case-insensitive")
        self.assertEqual(len(probe.find_nodes(self.tree, "text_input", "Filter links")), 1)
        self.assertEqual(len(probe.find_nodes(self.tree, None, "Filter")), 1)
        self.assertEqual(probe.find_nodes(self.tree, "Button", "Capture"), [])
        self.assertEqual(probe.find_nodes({"nodes": {}}, "Label"), [])


class A11yStateAndOrder(unittest.TestCase):
    """Fix round 1 (R-j): state flags, document order, nth and sequence queries.

    The tree mirrors gpui-pre 0.3.7's node_to_json: ephemeral ids in AccessKit update
    order (the window `e` comes last), `root`, `gpui_focus`, booleans for selected and
    expanded, Debug-formatted toggled, and an `on_action` list that is NOT text.
    """

    tree = {
        "root": "e",
        "gpui_focus": "c",
        "active_descendant_focus": None,
        "nodes": {
            "a": {"aria": {"role": "ListItem", "label": "gamma.md", "selected": False, "on_action": ["Click"]}},
            "b": {"aria": {"role": "ListItem", "label": "alpha.md", "selected": False}},
            "c": {"aria": {"role": "ListItem", "label": "beta.md", "selected": True}},
            "d": {"aria": {"role": "Switch", "label": "Launch at login", "toggled": "True"}},
            "f": {"aria": {"role": "List"}, "children": ["b", "c", "a"]},
            "e": {"aria": {"role": "Window"}, "children": ["f", "d"]},
        },
    }

    def test_document_order_follows_root_and_children(self):
        nodes = probe.a11y_nodes(self.tree)
        self.assertEqual([n["id"] for n in nodes], ["e", "f", "b", "c", "a", "d"])

    def test_state_fields_are_kept_and_normalised(self):
        by_id = {n["id"]: n for n in probe.a11y_nodes(self.tree)}
        self.assertIs(by_id["c"]["state"]["selected"], True)
        self.assertIs(by_id["c"]["state"]["focused"], True)
        self.assertIs(by_id["b"]["state"]["focused"], False)
        self.assertIs(by_id["d"]["state"]["toggled"], True)
        self.assertEqual(by_id["a"]["state"]["actions"], ["Click"])
        # Action names are not text: "Click" must not satisfy a text query.
        self.assertEqual(probe.find_nodes(self.tree, text="Click"), [])

    def test_state_on_a_node_query(self):
        hits = probe.find_nodes(self.tree, "ListItem", state={"selected": True})
        self.assertEqual([n["texts"] for n in hits], [["beta.md"]])
        # A state the node does not report is null: `false` does not match it.
        self.assertEqual(probe.find_nodes(self.tree, "Switch", state={"selected": False}), [])
        self.assertEqual(len(probe.find_nodes(self.tree, "Switch", state={"selected": {"ne": True}})), 1)

    def test_nth_query(self):
        probe.check_a11y_query(self.tree, {"role": "ListItem", "nth": 1, "text": "beta", "state": {"selected": True}})
        probe.check_a11y_query(self.tree, {"role": "ListItem", "nth": -1, "text": "gamma"})
        with self.assertRaisesRegex(probe.StepFailed, "#0 mismatch"):
            probe.check_a11y_query(self.tree, {"role": "ListItem", "nth": 0, "state": {"selected": True}})
        with self.assertRaisesRegex(probe.StepFailed, "no ListItem node #3"):
            probe.check_a11y_query(self.tree, {"role": "ListItem", "nth": 3})

    def test_sequence_query(self):
        seq = lambda texts, mode: {"sequence": {"role": "ListItem", "texts": texts, "mode": mode}}  # noqa: E731
        probe.check_a11y_query(self.tree, seq(["alpha", "beta"], "prefix"))
        probe.check_a11y_query(self.tree, seq(["alpha", "beta", "gamma"], "exact"))
        probe.check_a11y_query(self.tree, seq(["alpha", "gamma"], "ordered"))
        for texts, mode in [
            (["beta", "alpha"], "prefix"),       # wrong order
            (["alpha", "beta"], "exact"),        # a third row exists
            (["gamma", "alpha"], "ordered"),     # alpha comes before gamma
            (["alpha", "beta", "gamma", "delta"], "prefix"),  # too few rows
        ]:
            with self.subTest(texts=texts, mode=mode), self.assertRaises(probe.StepFailed):
                probe.check_a11y_query(self.tree, seq(texts, mode))


class NoVacuousPasses(unittest.TestCase):
    """Fix round 1: nothing a scenario says may be satisfied by checking nothing."""

    def load(self, doc):
        with tempfile.TemporaryDirectory() as d:
            p = Path(d) / "s.json"
            p.write_text(json.dumps(doc))
            return probe.load_scenario(str(p))

    def test_empty_text_never_matches(self):
        self.assertFalse(probe.text_matches("", "anything"))
        self.assertFalse(probe.text_matches("   ", "anything"))
        self.assertFalse(probe.text_matches("", ""))

    def test_empty_queries_and_texts_are_refused(self):
        bad = [
            {"see": []},
            {"see": {}},
            {"see": {"text": ""}},
            {"see": {"text": "   "}},
            {"see": {"role": ""}},
            {"see": {"role": "Label", "state": {}}},
            {"see": {"role": "Button", "state": {"disabled": False}}},
            {"see": {"role": "Button", "state": {"selcted": True}}},
            {"see": {"text": "x", "nth": 0}},  # nth needs a role
            {"see": {"role": "ListItem", "nth": 0}, "absent": True},
            {"see": {"sequence": {"role": "ListItem", "texts": []}}},
            {"see": {"sequence": {"role": "ListItem", "texts": ["a"], "mode": "sorted"}}},
            {"see": {"sequence": {"role": "ListItem", "texts": ["a"]}}, "count": 1},
            {"ocr": []},
            {"ocr": ""},
            {"ocr": ["ok", " "]},
            {"graph": True, "links": []},
            {"graph": True, "no_links": [[]]},
            {"graph": True, "links": [["a:1", ""]]},
            {"graph": True, "link_count": -1},
            {"pasteboard": "read"},
            {"call": "state", "expect": {"x": {"text": ""}}},
            {"call": "state", "expect": {"x": {"contains": ""}}},
            {"file": ""},
        ]
        for step in bad:
            with self.subTest(step=step), self.assertRaises(probe.ScenarioError):
                self.load({"steps": [step]})

    def test_absent_on_an_empty_tree_fails(self):
        class Inst:
            root = data_dir = fixtures_dir = Path("/nonexistent")
            name = "t"

            def __init__(self, tree):
                self.tree = tree

            def call(self, verb, **args):
                return self.tree

        step = {"see": {"text": "Error"}, "absent": True, "timeout_ms": 0}
        for tree in ({"nodes": {}}, {"root": "a", "nodes": {"a": {"aria": {"role": "Window"}}}}):
            with self.subTest(tree=tree), self.assertRaisesRegex(probe.StepFailed, "rendered view"):
                probe.run_see(probe.RunContext(Inst(tree), {}), step)
        rendered = {"root": "a", "nodes": {"a": {"aria": {"role": "Window"}, "children": ["b"]},
                                           "b": {"aria": {"role": "Label", "value": "Capture"}}}}
        self.assertIn("absent", probe.run_see(probe.RunContext(Inst(rendered), {}), step))


class BuildProvenance(unittest.TestCase):
    """Fix round 1: the run owns the exact binary it built, and names its source."""

    def test_executable_comes_from_cargo_json(self):
        out = "\n".join([
            json.dumps({"reason": "compiler-artifact", "target": {"name": "serde", "kind": ["lib"]},
                        "executable": None}),
            "not json",
            json.dumps({"reason": "compiler-artifact", "target": {"name": "ubilink-next", "kind": ["bin"]},
                        "executable": "/x/target/debug/ubilink-next", "fresh": True}),
            json.dumps({"reason": "build-finished", "success": True}),
        ])
        self.assertEqual(probe.executable_from_cargo_json(out), "/x/target/debug/ubilink-next")
        self.assertIsNone(probe.executable_from_cargo_json('{"reason": "build-finished"}'))

    def test_source_revision_reports_dirty_build_inputs_only(self):
        with tempfile.TemporaryDirectory() as d:
            git = lambda *a: subprocess.run(["git", *a], cwd=d, check=True, capture_output=True)  # noqa: E731
            git("init", "-q")
            git("config", "user.email", "t@t")
            git("config", "user.name", "t")
            (Path(d) / "app").mkdir()
            (Path(d) / "app" / "lib.rs").write_text("1")
            git("add", ".")
            git("commit", "-qm", "c")
            head, dirty = probe.source_revision(Path(d))
            self.assertRegex(head, r"^[0-9a-f]{40}$")
            self.assertFalse(dirty)
            (Path(d) / "scratch.txt").write_text("not a build input")
            self.assertFalse(probe.source_revision(Path(d))[1])
            (Path(d) / "app" / "lib.rs").write_text("2")
            self.assertTrue(probe.source_revision(Path(d))[1])

    def test_build_copies_the_executable_while_holding_the_target_lock(self):
        with tempfile.TemporaryDirectory() as d:
            d = Path(d)
            cache, run_dir, bindir = d / "cache", d / "run", d / "bin"
            for x in (cache, run_dir, bindir):
                x.mkdir()
            exe = d / "built-ubilink-next"
            exe.write_bytes(b"binary from this commit")
            probe_log = d / "lock-seen"
            # A fake cargo: records whether target.lock is held while it "builds",
            # then reports the artifact the way cargo's JSON does.
            fake = bindir / "cargo"
            fake.write_text(textwrap.dedent(f"""\
                #!{sys.executable}
                import fcntl, json, os
                fd = os.open({str(cache / 'target.lock')!r}, os.O_RDWR)
                try:
                    fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
                    held = "free"
                except BlockingIOError:
                    held = "held"
                open({str(probe_log)!r}, "w").write(held)
                print(json.dumps({{"reason": "compiler-artifact",
                                  "target": {{"name": "ubilink-next", "kind": ["bin"]}},
                                  "executable": {str(exe)!r}}}))
            """))
            fake.chmod(0o755)
            saved = (probe.CACHE_ROOT, os.environ["PATH"], dict(probe._builds), probe._GPUI_SOURCE_TARGET)
            probe.CACHE_ROOT = cache
            probe._GPUI_SOURCE_TARGET = d / "no-target"  # don't clone the real 5 GB target
            os.environ["PATH"] = f"{bindir}:{os.environ['PATH']}"
            probe._builds.clear()
            try:
                b = probe.build_binary(run_dir, out=lambda m: None)
            finally:
                probe.CACHE_ROOT, os.environ["PATH"], probe._GPUI_SOURCE_TARGET = saved[0], saved[1], saved[3]
                probe._builds.clear()
                probe._builds.update(saved[2])
            self.assertEqual(probe_log.read_text(), "held", "cargo ran under the target lock")
            self.assertEqual(b.path.parent, run_dir)
            self.assertEqual(b.path.read_bytes(), b"binary from this commit")
            # The run's copy is independent of the shared target: replacing the shared
            # executable afterwards (another clone's build) does not change ours.
            exe.write_bytes(b"someone else's commit")
            self.assertEqual(b.path.read_bytes(), b"binary from this commit")
            self.assertEqual(b.sha256, probe._sha256_file(b.path))
            self.assertIn(f"sha256={b.sha256}", b.describe())
            self.assertRegex(b.describe(), r"commit=[0-9a-f]{40}|commit=unknown")


class ReleaseInstance(unittest.TestCase):
    def test_release_instance_has_isolation_but_no_channel(self):
        inst = probe.Instance("perf", Path("/bin/true"), release=True)
        inst.root = Path("/tmp/ubprobe-r")
        env = inst.env()
        self.assertEqual(env["UBILINK_DATA_DIR"], "/tmp/ubprobe-r/data")
        self.assertEqual(env["UBILINK_SKIP_HOTKEYS"], "1")
        with self.assertRaisesRegex(probe.ProbeError, "no automation channel"):
            inst.call("state")

    def test_release_instance_launches_and_stops(self):
        # A stand-in "release binary" that creates its single-instance socket path
        # the way ubilink-next does before GPUI starts, then idles.
        with tempfile.TemporaryDirectory() as d:
            fake = Path(d) / "fake-release"
            fake.write_text(textwrap.dedent("""\
                #!/bin/sh
                : > "$UBILINK_INSTANCE_SOCKET"
                [ -d "$UBILINK_DATA_DIR" ] || exit 7
                exec sleep 30
            """))
            fake.chmod(0o755)
            inst = probe.Instance("rel", fake, release=True)
            inst.start(timeout=10)
            try:
                self.assertIsNone(inst.proc.poll())
                root = inst.root
            finally:
                inst.stop()
            self.assertIsNotNone(inst.proc.poll())
            self.assertFalse(root.exists())


class Colour(unittest.TestCase):
    def test_delta_e(self):
        self.assertEqual(probe.delta_e((10, 20, 30), (10, 20, 30)), 0)
        self.assertLess(probe.delta_e((128, 128, 128), (129, 128, 128)), 1)
        self.assertGreater(probe.delta_e((0, 0, 0), (255, 255, 255)), 99)
        self.assertEqual(probe.parse_hex("#1E1e1e"), (30, 30, 30))
        with self.assertRaises(probe.ScenarioError):
            probe.parse_hex("red")

    def test_geometry(self):
        disp = {"x": 0, "y": 0, "w": 1440, "h": 900}
        self.assertTrue(probe.rect_within({"x": 400, "y": 204, "w": 640, "h": 140}, disp))
        self.assertFalse(probe.rect_within({"x": 1000, "y": 0, "w": 640, "h": 140}, disp))
        ds = [
            {"id": 3, "main": False, "bounds": {"x": -1920, "y": 0, "w": 1920, "h": 1080}},
            {"id": 1, "main": True, "bounds": disp},
            {"id": 2, "main": False, "bounds": {"x": 1440, "y": 0, "w": 1920, "h": 1080}},
        ]
        self.assertEqual([d["id"] for d in probe.order_displays(ds)], [1, 3, 2])


class ScenarioValidation(unittest.TestCase):
    def load(self, doc):
        with tempfile.TemporaryDirectory() as d:
            p = Path(d) / "s.json"
            p.write_text(json.dumps(doc))
            return probe.load_scenario(str(p))

    def test_shell_scenario_is_valid(self):
        sc = probe.load_scenario("shell")
        self.assertEqual(sc["_name"], "shell")
        kinds = [probe.step_type(s, "") for s in sc["steps"]]
        self.assertIn("see", kinds)
        self.assertIn("winframe", kinds)

    def test_winframe_gone_needs_an_earlier_window(self):
        self.load({"steps": [{"winframe": "gone", "timeout_ms": 500}]})
        with self.assertRaises(probe.StepFailed):
            probe.window_gone(None, [])
        on = [{"number": 7}, {"number": 9}]
        self.assertFalse(probe.window_gone(7, on))
        self.assertTrue(probe.window_gone(8, on))
        self.assertTrue(probe.window_gone(8, []))

    def test_every_step_type_has_a_runner(self):
        self.assertEqual(set(probe.STEP_TYPES), set(probe.STEP_RUNNERS))

    def test_unknown_step_is_a_scenario_error(self):
        with self.assertRaisesRegex(probe.ScenarioError, "unknown step"):
            self.load({"steps": [{"click": "here"}]})
        with self.assertRaisesRegex(probe.ScenarioError, "unknown step"):
            self.load({"steps": [{"call": "state", "see": {"text": "x"}}]})

    def test_typoed_option_is_refused(self):
        with self.assertRaisesRegex(probe.ScenarioError, "expcet"):
            self.load({"steps": [{"call": "state", "expcet": {"x": 1}}]})

    def test_bad_values_are_refused(self):
        bad = [
            {"call": "trigger_hotkey", "retry_ms": 100},  # retry only on read-only verbs
            {"see": {"colour": "red"}},
            {"pixel": {"x": 1, "y": 1}},  # no color
            {"pixel": {"x": 1, "y": 1}, "color": "nope"},
            {"pasteboard": "wipe"},
            {"graph": True},
            {"winframe": "left"},
            {"warp_cursor": "main"},
            {"restart": "now"},
            {"sleep_ms": -1},
            {"call": "state", "expect": {"x": {"type": "tuple"}}},
            {"call": "state", "expect": {"x": {"matches": "("}}},
        ]
        for step in bad:
            with self.subTest(step=step), self.assertRaises(probe.ScenarioError):
                self.load({"steps": [step]})

    def test_file_shape(self):
        with self.assertRaisesRegex(probe.ScenarioError, "non-empty"):
            self.load({"steps": []})
        with self.assertRaisesRegex(probe.ScenarioError, "unknown key"):
            self.load({"steps": [{"restart": True}], "setpu": {}})
        with self.assertRaisesRegex(probe.ScenarioError, "not found"):
            probe.load_scenario("no-such-scenario")

    def test_expand(self):
        t = {"root": "/r", "nonce": "n1"}
        self.assertEqual(
            probe.expand({"p": "{root}/x", "l": ["{nonce}", 3], "{root}": True}, t),
            {"p": "/r/x", "l": ["n1", 3], "/r": True},
        )


class Instances(unittest.TestCase):
    def test_instance_names_fit_the_apps_rule(self):
        n = probe.instance_name("link-browser-filter-2", pid=4194304)
        self.assertRegex(n, r"^[A-Za-z0-9_-]{1,32}$")
        self.assertEqual(probe.instance_name("a b/c", pid=7), "p7-a-b-c")

    def test_env_isolates_and_skips_hotkeys(self):
        inst = probe.Instance("t", Path("/bin/true"))
        inst.root = Path("/tmp/ubprobe-x")
        os.environ["UBILINK_SOMETHING_INHERITED"] = "1"
        try:
            env = inst.env()
        finally:
            del os.environ["UBILINK_SOMETHING_INHERITED"]
        self.assertEqual(env["UBILINK_SKIP_HOTKEYS"], "1")
        self.assertEqual(env["UBILINK_INSTANCE"], inst.name)
        self.assertEqual(env["UBILINK_DATA_DIR"], "/tmp/ubprobe-x/data")
        self.assertEqual(env["UBILINK_INSTANCE_SOCKET"], "/tmp/ubprobe-x/si.sock")
        self.assertNotIn("UBILINK_SOMETHING_INHERITED", env)
        self.assertTrue(str(inst.socket_path).endswith(f"/{inst.name}/auto.sock"))


class Graph(unittest.TestCase):
    def test_reads_sqlite_and_export(self):
        with tempfile.TemporaryDirectory() as d:
            db = Path(d) / "ubilink-graph.sqlite"
            conn = sqlite3.connect(db)
            conn.executescript(
                """
                CREATE TABLE resources (id TEXT PRIMARY KEY, name TEXT NOT NULL, uri TEXT NOT NULL);
                CREATE TABLE links (id TEXT PRIMARY KEY, lo_id TEXT NOT NULL, hi_id TEXT NOT NULL,
                                    UNIQUE (lo_id, hi_id));
                INSERT INTO resources VALUES ('a:1','A','a:1'), ('b:2','B','b:2'), ('c:3','C','c:3');
                INSERT INTO links VALUES ('l1','a:1','b:2');
                """
            )
            conn.commit()
            conn.close()
            links, n = probe.read_graph(db=db)
            self.assertEqual(n, 3)
            self.assertIn(frozenset(("b:2", "a:1")), links, "links are unordered pairs")
            self.assertNotIn(frozenset(("a:1", "c:3")), links)

            exp = Path(d) / "export.json"
            exp.write_text(json.dumps({
                "schema": "ubilink.graph.v3",
                "resources": [{"guid": "g1", "uri": "a:1"}, {"guid": "g2", "uri": "b:2"}],
                "links": [{"a": "g2", "b": "g1", "created_at": None}],
            }))
            links, n = probe.read_graph(export=exp)
            self.assertEqual((links, n), ({frozenset(("a:1", "b:2"))}, 2))


class Subsequence(unittest.TestCase):
    """`is_subsequence` (Task 10): the match rule ranking-golden.json's `expect_top3`
    and `decoys` both lean on, and R5's "rank in-order subsequence matches above
    other matches" wire-level contract."""

    def test_in_order_characters_match_with_gaps(self):
        self.assertTrue(probe.is_subsequence("gthb", "github.com"))
        self.assertTrue(probe.is_subsequence("GTHB", "https://github.com/ubilink"), "case-insensitive")
        self.assertTrue(probe.is_subsequence("abc", "abc"), "exact match is a subsequence of itself")

    def test_out_of_order_or_missing_characters_do_not_match(self):
        self.assertFalse(probe.is_subsequence("bthg", "github.com"), "right letters, wrong order")
        self.assertFalse(probe.is_subsequence("gthbz", "github.com"), "a letter github.com lacks")

    def test_empty_query_never_matches(self):
        self.assertFalse(probe.is_subsequence("", "anything"))
        self.assertFalse(probe.is_subsequence("", ""))


class MinWindowSpanAndRank(unittest.TestCase):
    """`min_window_span`/`rank` (Task 10 fix round 1): the reference ranking rule
    fixtures/README.md documents as normative. `min_window_span` must find the
    true MINIMUM window, not the greedy-leftmost one `is_subsequence`'s own scan
    would stop at — the whole reason `rank` cannot be built on `is_subsequence`
    alone."""

    def test_greedy_leftmost_and_minimum_window_differ(self):
        # The greedy left-to-right scan matches 'a' at index 0, then 'b' at index
        # 2 -- a 3-character span ("aab"). The true minimum window is "ab" at
        # indices 1-2 (span 2): a tighter embedding using the SECOND 'a'.
        self.assertEqual(probe.min_window_span("ab", "aabb"), (2, 1))

    def test_no_match_returns_none(self):
        self.assertIsNone(probe.min_window_span("ba", "ab"), "wrong order")
        self.assertIsNone(probe.min_window_span("abz", "ab"), "a letter missing entirely")

    def test_exact_match_spans_the_whole_text(self):
        self.assertEqual(probe.min_window_span("abc", "abc"), (3, 0))

    def test_ties_prefer_the_earliest_start(self):
        # "ab" embeds at (0,1) and (3,4), both span 2; the earliest start wins.
        self.assertEqual(probe.min_window_span("ab", "ab_ab"), (2, 0))

    # Names below are deliberately non-matching filler ("Page") so these cases
    # isolate ranking behaviour on the uri field alone; the next block covers
    # the name field and the OR-across-fields/field-priority rules directly.

    def test_rank_orders_by_span_then_start_then_uri(self):
        # "xy" is tight (span 2) in the first uri, loose (span 4) in the second:
        # span is the primary key, so the tight one ranks first regardless of
        # uri ordering.
        items = [("Page", "https://b.example/w-x-y-z"), ("Page", "https://a.example/xy")]
        self.assertEqual(probe.rank("xy", items), ["https://a.example/xy", "https://b.example/w-x-y-z"])

    def test_rank_tie_breaks_on_uri_ascending_when_span_and_start_tie(self):
        items = [("Page", "https://zzz.example/ab"), ("Page", "https://aaa.example/ab")]
        self.assertEqual(
            probe.rank("ab", items), ["https://aaa.example/ab", "https://zzz.example/ab"]
        )

    def test_rank_drops_non_matches_and_is_case_insensitive(self):
        items = [("Page", "https://GitHub.com/x"), ("Page", "https://nomatch.example/zzz")]
        self.assertEqual(probe.rank("gthb", items), ["https://GitHub.com/x"])

    def test_rank_does_not_depend_on_corpus_order(self):
        items = [("Page", "https://a.example/gthb"), ("Page", "https://b.example/githubbbb")]
        self.assertEqual(probe.rank("gthb", items), probe.rank("gthb", list(reversed(items))))


class ItemWindowTwoFieldRule(unittest.TestCase):
    """`item_window` (Task 10 fix round 2, controller ruling R-m): an item
    matches on `name` OR `uri`; the better (smaller) span wins, with `name`
    breaking an exact span-and-start tie — required for parity with the Tauri
    browser's filter, which already searches `${name} ${uri}`
    (dist/browse-state.mjs)."""

    def test_matches_on_name_when_uri_does_not_match_at_all(self):
        # A Bear note: the title carries the match, its uri is an opaque id
        # with no 'g' at all (so it cannot match "trgn" by any embedding).
        got = probe.item_window("trgn", "Triage inbox notes", "bear://x-callback-url/open-note?id=7f3a9c21")
        self.assertEqual(got, (9, 0, probe.NAME_FIELD))

    def test_matches_on_uri_when_name_does_not_match_at_all(self):
        got = probe.item_window("gthb", "Opaque title", "https://github.com/x")
        self.assertEqual(got[2], probe.URI_FIELD)

    def test_no_match_on_either_field_is_none(self):
        self.assertIsNone(probe.item_window("zzzz", "no letters here", "https://nomatch.example/page"))

    def test_the_tighter_field_wins_even_when_the_other_field_also_matches(self):
        # Both fields match "rddt" ("Reddit discussion" / "https://rddt.example/..."),
        # but the uri's literal "rddt" (span 4) is tighter than the name's
        # "Reddit" (span 6) -- the uri field wins despite name's tie-break
        # priority, because span is compared BEFORE field priority.
        got = probe.item_window("rddt", "Reddit discussion", "https://rddt.example/thread/99231")
        self.assertEqual(got, (4, 8, probe.URI_FIELD))

    def test_name_wins_an_exact_span_and_start_tie(self):
        # Both fields embed "ab" at the very start, span 2 -- an exact tie
        # (span AND start), so field priority (name before uri) decides.
        got = probe.item_window("ab", "ab-name", "ab-uri")
        self.assertEqual(got, (2, 0, probe.NAME_FIELD))

    def test_rank_result_is_unaffected_by_which_field_won_a_tie(self):
        # Field priority only orders the two fields of ONE item; it must never
        # leak into the cross-item comparison `rank` performs (which compares
        # only span/start/uri, not field) -- two items tied on span+start but
        # differing in which field won must still tie on `rank`'s own key.
        name_wins = probe.item_window("ab", "ab-name", "ab-uri")
        uri_wins = probe.item_window("ab", "zz-name", "ab-uri")
        self.assertEqual(name_wins[:2], uri_wins[:2], "span/start equal regardless of which field won")


class FixturesGenerator(unittest.TestCase):
    """`generate_graph_5000` (Task 10, R4/R5): must be exactly reproducible from its
    seed, since `--check` compares the committed file against a fresh run."""

    def test_deterministic_from_seed(self):
        a = probe.generate_graph_5000(seed=1, count=50)
        b = probe.generate_graph_5000(seed=1, count=50)
        self.assertEqual(a, b)

    def test_different_seed_reshuffles_capture_order(self):
        a = probe.generate_graph_5000(seed=1, count=50)
        b = probe.generate_graph_5000(seed=2, count=50)
        self.assertNotEqual(
            [r["created_at"] for r in a["resources"]],
            [r["created_at"] for r in b["resources"]],
        )

    def test_shape(self):
        doc = probe.generate_graph_5000(seed=1, count=200)
        self.assertEqual(doc["schema"], "ubilink.graph.v3")
        self.assertEqual(doc["links"], [])
        self.assertEqual(len(doc["resources"]), 200)
        uris = {r["uri"] for r in doc["resources"]}
        guids = {r["guid"] for r in doc["resources"]}
        self.assertEqual(len(uris), 200, "every uri must be unique")
        self.assertEqual(len(guids), 200, "every guid must be unique")


class FixturesValidators(unittest.TestCase):
    """Unit tests for the fixture validators (Task 10) against constructed
    documents — independent of whatever is currently committed, so a validator
    regression is caught even if the fixture it checks happens to still pass."""

    def test_graph_shape_catches_bad_schema_dangling_link_self_link_and_dup_uri(self):
        self.assertIn("schema", probe._validate_graph_shape({"schema": "nope"})[0])
        doc = {
            "schema": "ubilink.graph.v3",
            "resources": [{"guid": "a", "name": "A", "uri": "x://a"}],
            "links": [{"a": "a", "b": "ghost"}],
        }
        issues = probe._validate_graph_shape(doc)
        self.assertTrue(any("ghost" in i for i in issues))

        self_link = {
            "schema": "ubilink.graph.v3",
            "resources": [{"guid": "a", "name": "A", "uri": "x://a"}],
            "links": [{"a": "a", "b": "a"}],
        }
        self.assertTrue(any("self-link" in i for i in probe._validate_graph_shape(self_link)))

        dup_uri = {
            "schema": "ubilink.graph.v3",
            "resources": [
                {"guid": "a", "name": "A", "uri": "x://same"},
                {"guid": "b", "name": "B", "uri": "x://same"},
            ],
            "links": [],
        }
        self.assertTrue(any("not unique" in i for i in probe._validate_graph_shape(dup_uri)))

        clean = {
            "schema": "ubilink.graph.v3",
            "resources": [
                {"guid": "a", "name": "A", "uri": "x://a"},
                {"guid": "b", "name": "B", "uri": "x://b"},
            ],
            "links": [{"a": "a", "b": "b", "created_at": 1}],
        }
        self.assertEqual(probe._validate_graph_shape(clean), [])

    def test_capture_delayed_requires_exactly_300ms(self):
        ok = {"name": "N", "uri": "u", "app": "A", "delay_ms": 300}
        self.assertEqual(probe._validate_capture_delayed(ok), [])
        wrong = {**ok, "delay_ms": 301}
        self.assertTrue(any("300" in i for i in probe._validate_capture_delayed(wrong)))

    def test_capture_failed_rejects_a_terse_or_mixed_shape(self):
        self.assertTrue(probe._validate_capture_failed({"error": "nope"}), "too terse for U3")
        self.assertEqual(
            probe._validate_capture_failed({"error": "Safari returned nothing, try again."}), []
        )
        mixed = {"error": "Safari returned nothing, try again.", "uri": "https://x"}
        self.assertTrue(any("uri" in i for i in probe._validate_capture_failed(mixed)))

    def test_permissions_two_missing_rejects_a_granted_permission(self):
        self.assertEqual(
            probe._validate_permissions_two_missing({"accessibility": False, "automation": "denied"}), []
        )
        self.assertTrue(
            probe._validate_permissions_two_missing({"accessibility": True, "automation": "denied"})
        )

    def test_backlinks_doc_catches_wrong_order_and_a_linked_decoy(self):
        def doc(order_ok=True, decoy_linked=False):
            links = [
                {"a": "subj", "b": "bl-a", "created_at": 1},
                {"a": "subj", "b": "bl-b", "created_at": 2},
                {"a": "subj", "b": "bl-c", "created_at": 3},
            ]
            if not order_ok:
                links[0]["created_at"], links[2]["created_at"] = links[2]["created_at"], links[0]["created_at"]
            if decoy_linked:
                links.append({"a": "subj", "b": "decoy", "created_at": 4})
            return {
                "schema": "ubilink.graph.v3",
                "resources": [
                    {"guid": "subj", "name": "S", "uri": probe._BACKLINKS_SUBJECT_URI, "created_at": 0},
                    {"guid": "bl-a", "name": "A", "uri": "https://alpha.example/one", "created_at": 0},
                    {"guid": "bl-b", "name": "B", "uri": "https://beta.example/two", "created_at": 0},
                    {"guid": "bl-c", "name": "C", "uri": "https://gamma.example/three", "created_at": 0},
                    {"guid": "decoy", "name": "D", "uri": probe._BACKLINKS_DECOY_URI, "created_at": 0},
                ],
                "links": links,
            }

        self.assertEqual(probe._validate_backlinks_doc(doc(), probe._BACKLINKS_ORDER_NEWEST_FIRST), [])
        self.assertTrue(
            probe._validate_backlinks_doc(doc(order_ok=False), probe._BACKLINKS_ORDER_NEWEST_FIRST)
        )
        issues = probe._validate_backlinks_doc(doc(decoy_linked=True), probe._BACKLINKS_ORDER_NEWEST_FIRST)
        self.assertTrue(any("decoy" in i for i in issues))

    def test_ranking_golden_catches_a_scorer_disagreement_and_a_matching_decoy(self):
        # A forward-ordered "uri" (every increasing-length prefix is trivially an
        # in-order subsequence of it) paired with its exact reverse (no forward pair
        # of two-or-more letters can be an in-order subsequence of it), so 20
        # distinct, genuinely valid {query, expect_top3, decoys} entries fall out of
        # one loop instead of 20 hand-picked ones.
        forward = "abcdefghijklmnopqrstuvwxyz"
        backward = forward[::-1]
        # name == uri for both items: this test is about disagreement/decoy
        # detection in general, not the two-field OR rule (ItemWindowTwoFieldRule
        # covers that), so there's nothing gained by giving them distinct fields.
        corpus = [(forward, forward), (backward, backward)]
        good = [
            {"query": forward[:n], "expect_top3": [forward], "decoys": [backward]}
            for n in range(2, 22)  # 20 distinct, increasing-length queries
        ]
        self.assertEqual(probe.rank(forward[:2], corpus), [forward], "sanity: only `forward` ever matches")
        self.assertEqual(probe._validate_ranking_golden(good, corpus), [])

        # expect_top3 names a uri that IS in the corpus but disagrees with what
        # `rank()` actually computes for that query (`backward` never matches at
        # all here, so this also doubles as a "wrong answer" rather than a
        # "missing from corpus" case).
        bad_expect = [dict(good[0], expect_top3=[backward])] + good[1:]
        issues = probe._validate_ranking_golden(bad_expect, corpus)
        self.assertTrue(any("disagrees with the reference scorer" in i for i in issues))

        # A decoy that the reference scorer actually ranks is not a valid decoy.
        bad_decoy = [dict(good[0], decoys=[forward])] + good[1:]
        issues = probe._validate_ranking_golden(bad_decoy, corpus)
        self.assertTrue(any("IS ranked" in i for i in issues))

    def test_adapters_catches_an_unsorted_or_mismatched_list(self):
        rows = json.loads(probe._ADAPTER_DEFAULTS_JSON.read_text(encoding="utf-8"))
        good = sorted(
            (
                {
                    "app": r["app"],
                    "builtin": bool(r.get("builtin", False)),
                    "user_defined": False,
                    "enabled": True,
                    "scheme": (r.get("schemes") or [None])[0],
                }
                for r in rows
            ),
            key=lambda r: r["app"].lower(),
        )
        self.assertEqual(probe._validate_adapters(good), [])

        unsorted_rows = list(reversed(good))
        self.assertTrue(any("sorted" in i for i in probe._validate_adapters(unsorted_rows)))

        wrong_builtin = [dict(good[0], builtin=not good[0]["builtin"])] + good[1:]
        self.assertTrue(any("must match" in i for i in probe._validate_adapters(wrong_builtin)))


class FixturesCommand(unittest.TestCase):
    """End-to-end: `fixtures --check` must pass against the fixtures actually
    committed under desktop-gpui/probes/fixtures/, and `--generate` must reproduce
    graph-5000.json exactly (Task 10's own verify step)."""

    def test_check_passes_against_the_committed_fixtures(self):
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            code = probe.cmd_fixtures(argparse.Namespace(check=True, generate=False))
        self.assertEqual(code, 0, buf.getvalue())
        for name in probe.FIXTURE_FILES:
            self.assertIn(f"PASS: fixtures — {name}", buf.getvalue())

    def test_generate_reproduces_the_committed_graph_5000(self):
        committed = json.loads((probe.FIXTURES_DIR / "graph-5000.json").read_text(encoding="utf-8"))
        self.assertEqual(committed, probe.generate_graph_5000())

    def test_check_fails_when_the_golden_table_disagrees_with_the_scorer(self):
        # A full copy of the real fixtures dir, with ranking-golden.json's first
        # entry's expect_top3 reversed (still every uri from the corpus, so this
        # exercises the "wrong order" path, not "missing from corpus") — proves
        # `--check` actually recomputes the ranking rather than trusting the file.
        with tempfile.TemporaryDirectory() as d:
            tmp_fixtures = Path(d) / "fixtures"
            shutil.copytree(probe.FIXTURES_DIR, tmp_fixtures)
            golden_path = tmp_fixtures / "ranking-golden.json"
            golden = json.loads(golden_path.read_text(encoding="utf-8"))
            entry = next(e for e in golden if len(e["expect_top3"]) > 1)
            entry["expect_top3"] = list(reversed(entry["expect_top3"]))
            golden_path.write_text(json.dumps(golden), encoding="utf-8")

            saved = probe.FIXTURES_DIR
            probe.FIXTURES_DIR = tmp_fixtures
            try:
                buf = io.StringIO()
                with contextlib.redirect_stdout(buf):
                    code = probe._fixtures_check()
            finally:
                probe.FIXTURES_DIR = saved
        self.assertEqual(code, 1)
        self.assertIn("disagrees with the reference scorer", buf.getvalue())
        self.assertIn("FAIL: fixtures — ranking-golden.json", buf.getvalue())


class EnvBlockedIsNotAFail(unittest.TestCase):
    def test_missing_screen_recording_blocks(self):
        saved = probe.quartz
        probe.quartz = lambda *a: {"granted": False}
        try:
            with self.assertRaisesRegex(probe.EnvBlocked, "Screen Recording"):
                probe._screen_access()
        finally:
            probe.quartz = saved

    def test_exit_codes(self):
        t = probe.Tally()
        self.assertEqual(t.exit_code(), 0)
        t.blocked = 1
        self.assertEqual(t.exit_code(), 3, "env-blocked alone is not a product FAIL")
        t.failed = 1
        self.assertEqual(t.exit_code(), 1)
        t.errors = 1
        self.assertEqual(t.exit_code(), 2)

    def test_run_steps_reports_blocked_and_continues(self):
        class FakeInst:
            def request(self, verb, args):
                return {"ok": True, "result": {"x": 1}}

        sc = {"steps": [{"warp_cursor": 7}, {"call": "state", "expect": {"x": 1}}]}
        saved = probe.quartz
        probe.quartz = lambda *a: [{"id": 1, "main": True, "bounds": {"x": 0, "y": 0, "w": 10, "h": 10}}]
        try:
            ctx = probe.RunContext(FakeInst(), sc)
            ctx.tokens = lambda: {}
            tally = probe.Tally()
            with contextlib.redirect_stdout(io.StringIO()) as out:
                probe.run_steps(ctx, tally, "t")
            self.assertIn("ENV-BLOCKED t [1/2]", out.getvalue())
        finally:
            probe.quartz = saved
        self.assertEqual((tally.blocked, tally.passed, tally.failed), (1, 1, 0))


class UnattendedGuards(unittest.TestCase):
    """Phase 1 R15 runs probes with nobody watching: environment trouble (display
    asleep, another app taking focus) must come out ENV-BLOCKED, not FAIL."""

    def test_display_asleep_is_env_blocked(self):
        with self.assertRaisesRegex(probe.EnvBlocked, "display asleep"):
            probe.displays_or_blocked([])
        one = [{"id": 1, "main": True, "bounds": {"x": 0, "y": 0, "w": 10, "h": 10}}]
        self.assertEqual(probe.displays_or_blocked(one), one)

    def test_nil_window_list_is_env_blocked_not_empty(self):
        saved = probe.quartz

        def nil(*a):
            raise probe.ProbeError("quartz windows 1: CGWindowListCopyWindowInfo returned nil")

        probe.quartz = nil
        try:
            with self.assertRaises(probe.EnvBlocked):
                probe.screen_windows(1)
        finally:
            probe.quartz = saved

    def test_focus_thief(self):
        term = {"pid": 10, "name": "iTerm2"}
        thief = {"pid": 30, "name": "Mail"}
        own = {1, 2}
        self.assertEqual(probe.focus_thief(term, thief, own), "Mail")
        self.assertIsNone(probe.focus_thief(term, term, own), "nothing changed")
        self.assertIsNone(probe.focus_thief(term, {"pid": 2, "name": "act"}, own), "own process")
        self.assertIsNone(probe.focus_thief(term, None, own), "unknown is not theft")
        self.assertEqual(probe.focus_thief(None, {"pid": 30}, own), "pid 30")

    def test_visibility_steps(self):
        vis = probe.is_visibility_step
        self.assertTrue(vis("winframe", {"winframe": "gone"}))
        self.assertTrue(vis("call", {"call": "state", "expect": {"shell.visible": False}}))
        self.assertTrue(vis("call", {"call": "state", "expect": {"route": None}}))
        self.assertTrue(vis("call", {"call": "resign_key"}))
        self.assertFalse(vis("call", {"call": "metrics", "expect": {"summary.x.count": 1}}))
        self.assertFalse(vis("call", {"call": "state", "expect": {"data_dir": "/x"}}))
        self.assertFalse(vis("see", {"see": {"text": "x"}}))

    def test_step_units_group_blocks(self):
        steps = [{"a": 1}, {"b": 1, "block": "x"}, {"c": 1, "block": "x"},
                 {"d": 1, "block": "y"}, {"e": 1}]
        units = [[i for i, _ in u] for u in probe.step_units(steps)]
        self.assertEqual(units, [[1], [2, 3], [4], [5]])

    def _run(self, fronts, results):
        """run_steps over one block with a fake instance and frontmost sequence."""
        calls = []

        class FakeInst:
            proc = None

            def request(self, verb, args):
                calls.append(verb)
                return {"ok": True, "result": results.pop(0) if verb == "state" and results else {}}

            def call(self, verb, **args):
                calls.append(f"reset:{verb}")
                return {"shell": {"visible": False}}

        sc = {"steps": [{"call": "state", "block": "b", "expect": {"shell.visible": True}}]}
        saved = probe.quartz
        probe.quartz = lambda *a: fronts.pop(0) if a[0] == "frontmost" else []
        try:
            ctx = probe.RunContext(FakeInst(), sc)
            ctx.tokens = lambda: {}
            tally = probe.Tally()
            with contextlib.redirect_stdout(io.StringIO()) as out:
                probe.run_steps(ctx, tally, "t")
        finally:
            probe.quartz = saved
        return tally, out.getvalue(), calls

    def test_focus_theft_retries_once_then_passes(self):
        term, mail = {"pid": 10, "name": "iTerm2"}, {"pid": 30, "name": "Mail"}
        hidden, shown = {"shell": {"visible": False}}, {"shell": {"visible": True}}
        tally, out, calls = self._run([term, mail, term], [hidden, shown])
        self.assertIn("RETRY", out)
        self.assertIn("focus taken by Mail", out)
        self.assertIn("reset:state", calls, "the panel is reset before the re-run")
        self.assertEqual((tally.passed, tally.failed, tally.blocked), (1, 0, 0))

    def test_focus_theft_twice_is_env_blocked(self):
        term, mail = {"pid": 10, "name": "iTerm2"}, {"pid": 30, "name": "Mail"}
        hidden = {"shell": {"visible": False}}
        tally, out, _ = self._run([term, mail, term, mail], [hidden, hidden])
        self.assertIn("ENV-BLOCKED", out)
        self.assertIn("focus taken by Mail (twice)", out)
        self.assertEqual((tally.passed, tally.failed, tally.blocked), (0, 0, 1))

    def test_failure_without_theft_is_a_fail(self):
        term = {"pid": 10, "name": "iTerm2"}
        tally, out, _ = self._run([term, term], [{"shell": {"visible": False}}])
        self.assertNotIn("RETRY", out)
        self.assertEqual((tally.passed, tally.failed, tally.blocked), (0, 1, 0))

    def test_activate_other_only_launches_the_probe_activator(self):
        sv = ScenarioValidation()
        sv.load({"steps": [{"activate_other": True}]})
        for bad in ("Finder", "Calculator", False):
            with self.subTest(bad=bad), self.assertRaises(probe.ScenarioError):
                sv.load({"steps": [{"activate_other": bad}]})
        self.assertTrue(probe._ACTIVATOR_SWIFT.is_file())


class GoneTracksTheCurrentPanel(unittest.TestCase):
    def test_visible_in_reads_both_result_shapes(self):
        self.assertTrue(probe.visible_in({"shell": {"visible": True}}))
        self.assertFalse(probe.visible_in({"state": {"shell": {"visible": False}}}))
        self.assertIsNone(probe.visible_in({"capacity": 3}))
        self.assertIsNone(probe.visible_in([]))

    def test_a_reopen_drops_the_old_window(self):
        class Inst:
            proc = None

        ctx = probe.RunContext(Inst(), {"steps": []})
        ctx.observe("window_id", {"window_id": 7})
        self.assertEqual(ctx.last_panel_wid, 7)
        ctx.observe("resign_key", {"state": {"shell": {"visible": False}}})
        self.assertEqual(ctx.last_panel_wid, 7, "a hide keeps it: `gone` checks this one")
        ctx.observe("trigger_hotkey", {"shell": {"visible": True}})
        self.assertIsNone(ctx.last_panel_wid, "a new panel: the old number must not be checked")
        ctx.observe("window_id", {"window_id": 9})
        ctx.observe("state", {"shell": {"visible": True}})
        self.assertEqual(ctx.last_panel_wid, 9, "still the same panel")
        with self.assertRaises(probe.StepFailed):
            probe.window_gone(None, [])


class DesktopLock(unittest.TestCase):
    def test_second_holder_waits(self):
        with tempfile.TemporaryDirectory() as d:
            lock = Path(d) / "desktop.lock"
            holder = textwrap.dedent(f"""
                import sys; sys.path.insert(0, {str(Path(probe.__file__).parent)!r})
                import probe, time
                with probe.desktop_lock({str(lock)!r}, timeout=5, out=lambda m: None):
                    print("held", flush=True)
                    time.sleep(1.5)
            """)
            proc = subprocess.Popen([sys.executable, "-c", holder], stdout=subprocess.PIPE, text=True)
            try:
                self.assertEqual(proc.stdout.readline().strip(), "held")
                with self.assertRaisesRegex(probe.ProbeError, "still held"):
                    with probe.desktop_lock(lock, timeout=0.3, out=lambda m: None):
                        pass
                # Once the holder exits the lock is free again.
                proc.wait(timeout=10)
                messages = []
                with probe.desktop_lock(lock, timeout=1, out=messages.append):
                    self.assertEqual(lock.read_text().strip(), str(os.getpid()))
                self.assertEqual(messages, [])
            finally:
                proc.kill()
                proc.wait()
                proc.stdout.close()


class WarmTarget(unittest.TestCase):
    def test_seed_clones_once(self):
        with tempfile.TemporaryDirectory() as d:
            src = Path(d) / "src-target"
            (src / "debug").mkdir(parents=True)
            (src / "debug" / "marker").write_text("x")
            cache = Path(d) / "cache" / "target"
            self.assertIn("seeded", probe._seed_target_cache(cache, src))
            self.assertEqual((cache / "debug" / "marker").read_text(), "x")
            self.assertEqual(probe._seed_target_cache(cache, src), "warm")
            missing = Path(d) / "other" / "target"
            self.assertIn("cold", probe._seed_target_cache(missing, Path(d) / "nope"))


class DoctorStatusAndOutput(unittest.TestCase):
    """`doctor_status` (aggregation) and `cmd_doctor` (output format), with every
    check mocked out - task 9's own unit tests, R9."""

    def test_pass_unless_some_check_failed(self):
        self.assertEqual(probe.doctor_status([]), "PASS")
        self.assertEqual(probe.doctor_status([{"status": "PASS"}, {"status": "WARN"}]), "PASS")
        self.assertEqual(
            probe.doctor_status([{"status": "PASS"}, {"status": "FAIL"}, {"status": "WARN"}]), "FAIL"
        )

    def test_text_output_one_line_per_check_then_result(self):
        checks = [
            {"name": "a", "status": "PASS", "detail": "ok"},
            {"name": "b", "status": "FAIL", "detail": "bad"},
        ]
        saved = probe._run_doctor_checks
        probe._run_doctor_checks = lambda build=False: checks
        try:
            with contextlib.redirect_stdout(io.StringIO()) as out:
                code = probe.cmd_doctor(argparse.Namespace(json=False, build=False))
        finally:
            probe._run_doctor_checks = saved
        lines = out.getvalue().splitlines()
        self.assertIn("PASS a: ok", lines)
        self.assertIn("FAIL b: bad", lines)
        self.assertEqual(lines[-1], "DOCTOR_RESULT: FAIL")
        self.assertEqual(code, 1)

    def test_json_output_is_machine_readable(self):
        checks = [{"name": "a", "status": "PASS", "detail": "ok"}]
        saved = probe._run_doctor_checks
        probe._run_doctor_checks = lambda build=False: checks
        try:
            with contextlib.redirect_stdout(io.StringIO()) as out:
                code = probe.cmd_doctor(argparse.Namespace(json=True, build=False))
        finally:
            probe._run_doctor_checks = saved
        doc = json.loads(out.getvalue())
        self.assertEqual(doc["result"], "PASS")
        self.assertEqual(doc["checks"], checks)
        self.assertEqual(code, 0)

    def test_a_crashing_check_is_reported_as_a_fail_not_an_exception(self):
        def boom():
            raise RuntimeError("kaboom")

        saved = probe._DOCTOR_CHECKS
        probe._DOCTOR_CHECKS = (boom,)
        try:
            checks = probe._run_doctor_checks(build=False)
        finally:
            probe._DOCTOR_CHECKS = saved
        # boom's own entry plus the always-run automation_build WARN.
        self.assertEqual(checks[0]["status"], "FAIL")
        self.assertIn("kaboom", checks[0]["detail"])


class DoctorChecks(unittest.TestCase):
    """Each check mocks only the helper it calls (quartz, swiftc, etc.), the same
    way the rest of this file mocks `probe.quartz`."""

    def test_screen_recording_granted(self):
        saved = probe.quartz
        probe.quartz = lambda *a: {"granted": True}
        try:
            c = probe.check_screen_recording()
        finally:
            probe.quartz = saved
        self.assertEqual(c, {"name": "screen_recording", "status": "PASS", "detail": "granted to this process"})

    def test_screen_recording_denied_reports_process_chain(self):
        saved_q, saved_chain = probe.quartz, probe._process_chain
        probe.quartz = lambda *a: {"granted": False}
        probe._process_chain = lambda pid, depth=6: ["python3", "zsh", "Terminal"]
        try:
            c = probe.check_screen_recording()
        finally:
            probe.quartz, probe._process_chain = saved_q, saved_chain
        self.assertEqual(c["status"], "FAIL")
        self.assertIn("python3 -> zsh -> Terminal", c["detail"])

    def test_display_awake(self):
        saved = probe.quartz
        probe.quartz = lambda *a: {"id": 1, "asleep": False}
        try:
            self.assertEqual(probe.check_display_awake()["status"], "PASS")
        finally:
            probe.quartz = saved

    def test_display_asleep_fails(self):
        saved = probe.quartz
        probe.quartz = lambda *a: {"id": 1, "asleep": True}
        try:
            c = probe.check_display_awake()
        finally:
            probe.quartz = saved
        self.assertEqual(c["status"], "FAIL")
        self.assertIn("asleep", c["detail"])

    def test_display_count_notes_single_display_as_informational(self):
        saved = probe.quartz
        probe.quartz = lambda *a: [{"id": 1, "main": True, "bounds": {}}]
        try:
            c = probe.check_display_count()
        finally:
            probe.quartz = saved
        self.assertEqual(c["status"], "PASS")
        self.assertIn("ENV-BLOCKED", c["detail"])

    def test_display_count_zero_fails(self):
        saved = probe.quartz
        probe.quartz = lambda *a: []
        try:
            c = probe.check_display_count()
        finally:
            probe.quartz = saved
        self.assertEqual(c["status"], "FAIL")

    def test_ocr_helper_pass_when_helper_compiles_and_runs(self):
        saved_helper, saved_run, saved_tiny = probe._helper, probe._run_helper, probe._tiny_png
        probe._helper = lambda source: Path("/fake/ocr-bin")
        probe._tiny_png = lambda path, **k: None
        probe._run_helper = lambda source, *a, **k: {"lines": []}
        try:
            c = probe.check_ocr_helper()
        finally:
            probe._helper, probe._run_helper, probe._tiny_png = saved_helper, saved_run, saved_tiny
        self.assertEqual(c["status"], "PASS")

    def test_ocr_helper_fail_when_compile_raises(self):
        saved = probe._helper

        def boom(source):
            raise probe.ProbeError("swiftc broke")

        probe._helper = boom
        try:
            c = probe.check_ocr_helper()
        finally:
            probe._helper = saved
        self.assertEqual(c["status"], "FAIL")
        self.assertIn("swiftc broke", c["detail"])

    def test_ocr_helper_fail_when_run_raises(self):
        saved_helper, saved_run, saved_tiny = probe._helper, probe._run_helper, probe._tiny_png
        probe._helper = lambda source: Path("/fake/ocr-bin")
        probe._tiny_png = lambda path, **k: None

        def boom(source, *a, **k):
            raise probe.ProbeError("vision exploded")

        probe._run_helper = boom
        try:
            c = probe.check_ocr_helper()
        finally:
            probe._helper, probe._run_helper, probe._tiny_png = saved_helper, saved_run, saved_tiny
        self.assertEqual(c["status"], "FAIL")
        self.assertIn("vision exploded", c["detail"])


class CaffeinateAssertionParsing(unittest.TestCase):
    SAMPLE = textwrap.dedent("""\
        Assertion status system-wide:
           PreventUserIdleDisplaySleep        1
        Listed by owning process:
            pid 4821(caffeinate): [0x4295000c2a] 00:00:01 PreventUserIdleDisplaySleep named: ""
            pid 100(Terminal): [0x4295000aaa] 00:10:00 PreventUserIdleSystemSleep named: "x"
        """)

    def test_finds_matching_pid_and_assertion_name(self):
        self.assertTrue(probe._pmset_has_assertion(self.SAMPLE, 4821, "PreventUserIdleDisplaySleep"))

    def test_rejects_wrong_pid(self):
        self.assertFalse(probe._pmset_has_assertion(self.SAMPLE, 9999, "PreventUserIdleDisplaySleep"))

    def test_rejects_right_pid_wrong_assertion_name(self):
        self.assertFalse(probe._pmset_has_assertion(self.SAMPLE, 100, "PreventUserIdleDisplaySleep"))


class CaffeinateCheck(unittest.TestCase):
    class FakeProc:
        def __init__(self, pid):
            self.pid = pid
            self._dead = False

        def poll(self):
            return 0 if self._dead else None

        def terminate(self):
            self._dead = True

        def kill(self):
            self._dead = True

        def wait(self, timeout=None):
            return 0

    def test_pass_when_assertion_seen(self):
        saved_start, saved_pm, saved_s = probe._start_caffeinate, probe._pmset_assertions, probe._CAFFEINATE_PROBE_S
        probe._start_caffeinate = lambda seconds: self.FakeProc(4821)
        probe._pmset_assertions = (
            lambda: 'pid 4821(caffeinate): [0x1] 00:00:01 PreventUserIdleDisplaySleep named: ""'
        )
        probe._CAFFEINATE_PROBE_S = 0.05
        try:
            c = probe.check_caffeinate()
        finally:
            probe._start_caffeinate, probe._pmset_assertions = saved_start, saved_pm
            probe._CAFFEINATE_PROBE_S = saved_s
        self.assertEqual(c["status"], "PASS")

    def test_fail_when_caffeinate_missing(self):
        saved = probe._start_caffeinate
        probe._start_caffeinate = lambda seconds: None
        try:
            c = probe.check_caffeinate()
        finally:
            probe._start_caffeinate = saved
        self.assertEqual(c["status"], "FAIL")

    def test_fail_when_assertion_never_appears(self):
        saved_start, saved_pm, saved_s = probe._start_caffeinate, probe._pmset_assertions, probe._CAFFEINATE_PROBE_S
        probe._start_caffeinate = lambda seconds: self.FakeProc(1)
        probe._pmset_assertions = lambda: "nothing relevant here"
        probe._CAFFEINATE_PROBE_S = 0.05
        try:
            c = probe.check_caffeinate()
        finally:
            probe._start_caffeinate, probe._pmset_assertions = saved_start, saved_pm
            probe._CAFFEINATE_PROBE_S = saved_s
        self.assertEqual(c["status"], "FAIL")


class XcodeAndRustChecks(unittest.TestCase):
    def test_swiftc_present(self):
        saved = probe._swiftc_version
        probe._swiftc_version = lambda: "Apple Swift version 6.0"
        try:
            c = probe.check_xcode_swiftc()
        finally:
            probe._swiftc_version = saved
        self.assertEqual(c["status"], "PASS")

    def test_swiftc_missing(self):
        saved = probe._swiftc_version
        probe._swiftc_version = lambda: ""
        try:
            c = probe.check_xcode_swiftc()
        finally:
            probe._swiftc_version = saved
        self.assertEqual(c["status"], "FAIL")

    def test_rust_toolchain_channel_parses_the_pin(self):
        with tempfile.TemporaryDirectory() as d:
            p = Path(d) / "rust-toolchain.toml"
            p.write_text('[toolchain]\nchannel = "1.98.1"\ncomponents = ["rustfmt"]\n')
            self.assertEqual(probe._rust_toolchain_channel(p), "1.98.1")

    def test_rust_toolchain_channel_missing_raises(self):
        with tempfile.TemporaryDirectory() as d:
            p = Path(d) / "rust-toolchain.toml"
            p.write_text("[toolchain]\n")
            with self.assertRaises(ValueError):
                probe._rust_toolchain_channel(p)

    def test_rust_toolchain_matches(self):
        saved_chan, saved_ver = probe._rust_toolchain_channel, probe._rustc_version
        probe._rust_toolchain_channel = lambda: "1.98.1"
        probe._rustc_version = lambda: "rustc 1.98.1 (abc123 2026-01-01)"
        try:
            c = probe.check_rust_toolchain()
        finally:
            probe._rust_toolchain_channel, probe._rustc_version = saved_chan, saved_ver
        self.assertEqual(c["status"], "PASS")

    def test_rust_toolchain_mismatch_fails(self):
        saved_chan, saved_ver = probe._rust_toolchain_channel, probe._rustc_version
        probe._rust_toolchain_channel = lambda: "1.98.1"
        probe._rustc_version = lambda: "rustc 1.90.0 (xyz 2024-01-01)"
        try:
            c = probe.check_rust_toolchain()
        finally:
            probe._rust_toolchain_channel, probe._rustc_version = saved_chan, saved_ver
        self.assertEqual(c["status"], "FAIL")

    def test_rust_toolchain_missing_rustc_fails(self):
        saved_chan, saved_ver = probe._rust_toolchain_channel, probe._rustc_version
        probe._rust_toolchain_channel = lambda: "1.98.1"
        probe._rustc_version = lambda: ""
        try:
            c = probe.check_rust_toolchain()
        finally:
            probe._rust_toolchain_channel, probe._rustc_version = saved_chan, saved_ver
        self.assertEqual(c["status"], "FAIL")


class AutomationBuildCheck(unittest.TestCase):
    def test_skipped_by_default_is_a_warn_not_a_fail(self):
        c = probe.check_automation_build(False)
        self.assertEqual(c["status"], "WARN")

    def test_build_failure_is_reported(self):
        saved = probe.build_binary

        def boom(run_dir, release=False, out=print):
            raise probe.ProbeError("cargo exploded")

        probe.build_binary = boom
        try:
            c = probe.check_automation_build(True)
        finally:
            probe.build_binary = saved
        self.assertEqual(c["status"], "FAIL")
        self.assertIn("cargo exploded", c["detail"])


class ProcessChain(unittest.TestCase):
    def test_walks_parents_until_pid_1(self):
        names = {10: "python3", 20: "zsh", 30: "Terminal"}
        parents = {10: 20, 20: 30, 30: 1}
        saved_name, saved_parent = probe._process_name, probe._parent_pid
        probe._process_name = lambda pid: names[pid]
        probe._parent_pid = lambda pid: parents.get(pid)
        try:
            chain = probe._process_chain(10)
        finally:
            probe._process_name, probe._parent_pid = saved_name, saved_parent
        self.assertEqual(chain, ["python3", "zsh", "Terminal"])

    def test_stops_at_depth(self):
        saved_name, saved_parent = probe._process_name, probe._parent_pid
        probe._process_name = lambda pid: f"p{pid}"
        probe._parent_pid = lambda pid: pid + 1
        try:
            chain = probe._process_chain(1000, depth=3)
        finally:
            probe._process_name, probe._parent_pid = saved_name, saved_parent
        self.assertEqual(len(chain), 3)


# ══════════════════════════════════════════════════════════════════════════
# Phase 1 Task 11: engine additions the 15 Phase 1 scenarios need
# ══════════════════════════════════════════════════════════════════════════


def _load(doc):
    with tempfile.TemporaryDirectory() as d:
        p = Path(d) / "s.json"
        p.write_text(json.dumps(doc))
        return probe.load_scenario(str(p))


class DeepAnyAndExactQueries(unittest.TestCase):
    """List rows keep their text in Label children (A11Y-NOTES.md), so row queries
    match text anywhere in the row's subtree; `any` expresses D6's 'enabled OR
    requires approval'; a query's own `exact` overrides the step's."""

    tree = {
        "root": "w",
        "nodes": {
            "w": {"aria": {"role": "Window"}, "children": ["l", "s1", "s2"]},
            "l": {"aria": {"role": "List"}, "children": ["r1", "r2"]},
            "r1": {"aria": {"role": "ListItem", "selected": True}, "children": ["n1", "u1"]},
            "n1": {"aria": {"role": "Label", "value": "Gamma Doc"}},
            "u1": {"aria": {"role": "Label", "value": "https://gamma.exa…"}},
            "r2": {"aria": {"role": "ListItem"}, "children": ["n2", "u2"]},
            "n2": {"aria": {"role": "Label", "value": "Beta Post"}},
            "u2": {"aria": {"role": "Label", "value": "https://beta.example/two"}},
            "s1": {"aria": {"role": "Label", "value": "Not granted"}},
            "s2": {"aria": {"role": "Label", "value": "Requires approval"}},
        },
    }

    def test_deep_matches_a_descendant_label_not_the_row_alone(self):
        q = {"role": "ListItem", "text": "https://beta.example/two"}
        with self.assertRaises(probe.StepFailed):
            probe.check_a11y_query(self.tree, q)
        probe.check_a11y_query(self.tree, {**q, "deep": True})
        # truncated uri label still matches on its own
        probe.check_a11y_query(self.tree, {"role": "ListItem", "nth": 0, "text": "https://gamma.example/three",
                                           "deep": True, "state": {"selected": True}})

    def test_deep_never_concatenates_two_labels(self):
        with self.assertRaises(probe.StepFailed):
            probe.check_a11y_query(self.tree, {"role": "ListItem", "text": "Beta Post https://beta", "deep": True})

    def test_deep_sequence(self):
        seq = {"sequence": {"role": "ListItem", "texts": ["Gamma Doc", "Beta Post"], "mode": "exact", "deep": True}}
        probe.check_a11y_query(self.tree, seq)
        with self.assertRaises(probe.StepFailed):
            probe.check_a11y_query(self.tree, {"sequence": {**seq["sequence"], "texts": ["Beta Post", "Gamma Doc"]}})
        with self.assertRaises(probe.StepFailed):  # not deep: ListItem nodes carry no text themselves
            probe.check_a11y_query(self.tree, {"sequence": {**seq["sequence"], "deep": False}})

    def test_query_exact_overrides_the_step(self):
        probe.check_a11y_query(self.tree, {"role": "Label", "text": "granted"})  # substring of "Not granted"
        with self.assertRaises(probe.StepFailed):
            probe.check_a11y_query(self.tree, {"role": "Label", "text": "granted", "exact": True})
        probe.check_a11y_query(self.tree, {"role": "Label", "text": "Not granted", "exact": True})

    def test_any_holds_when_one_alternative_fully_holds(self):
        enabled = [{"role": "Label", "text": "Enabled", "exact": True}]
        approval = [{"role": "Label", "text": "Requires approval", "exact": True},
                    {"role": "Label", "text": "Not granted"}]
        probe.check_a11y_query(self.tree, {"any": [enabled, approval]})
        half = [{"role": "Label", "text": "Requires approval"}, {"role": "Label", "text": "Login Items"}]
        with self.assertRaisesRegex(probe.StepFailed, "no alternative"):
            probe.check_a11y_query(self.tree, {"any": [enabled, half]})

    def test_validation(self):
        bad = [
            {"see": {"any": [[{"text": "a"}]]}},                       # one alternative
            {"see": {"any": [[{"text": "a"}], []]}},                   # empty alternative
            {"see": {"any": [[{"text": "a"}], [{"any": [[{"text": "b"}], [{"text": "c"}]]}]]}},  # nested
            {"see": {"any": [[{"text": "a"}], [{"text": "b"}]]}, "absent": True},
            {"see": {"role": "ListItem", "deep": True}},               # deep needs text
            {"see": {"role": "ListItem", "text": "x", "deep": "yes"}},
            {"see": {"role": "Label", "exact": True}},                 # exact needs text
            {"see": {"sequence": {"role": "ListItem", "texts": ["a"], "deep": 1}}},
        ]
        for step in bad:
            with self.subTest(step=step), self.assertRaises(probe.ScenarioError):
                _load({"steps": [step]})
        _load({"steps": [{"see": {"any": [[{"text": "a"}], [{"text": "b", "exact": True}]]}}]})


class FakeA11yInstance:
    """Answers `a11y` with a tree whose focused Switch moves one node per `key`."""

    def __init__(self, labels, fail_key=False):
        self.labels, self.focus, self.keys, self.fail_key = labels, 0, [], fail_key
        self.root = self.data_dir = self.fixtures_dir = Path("/nonexistent")
        self.name = "t"

    def tree(self):
        nodes = {"w": {"aria": {"role": "Window"}, "children": [f"s{i}" for i in range(len(self.labels))]}}
        for i, lab in enumerate(self.labels):
            nodes[f"s{i}"] = {"aria": {"role": "Switch", "label": lab, "toggled": "False"}}
        return {"root": "w", "gpui_focus": f"s{self.focus}", "nodes": nodes}

    def call(self, verb, **args):
        if verb == "a11y":
            return self.tree()
        if verb == "state":
            return {"route": "settings"}
        raise AssertionError(verb)

    def request(self, verb, args):
        assert verb == "key"
        self.keys.append(args["keys"])
        if self.fail_key:
            return {"ok": False, "error": "no panel open"}
        self.focus = (self.focus + 1) % len(self.labels)
        return {"ok": True, "result": [{"key": args["keys"], "handled": True}]}


class KeyUntil(unittest.TestCase):
    def run_step(self, inst, step):
        return probe.run_key_until(probe.RunContext(inst, {}), step)

    def test_presses_until_the_target_is_focused(self):
        inst = FakeA11yInstance(["Arc", "Bear", "Opera"])
        out = self.run_step(inst, {"key_until": {"role": "Switch", "text": "Opera", "state": {"focused": True}}})
        self.assertIn("after 2 press(es) of tab", out)
        self.assertEqual(inst.keys, ["tab", "tab"])

    def test_zero_presses_when_already_there(self):
        inst = FakeA11yInstance(["Arc", "Bear"])
        self.run_step(inst, {"key_until": {"role": "Switch", "text": "Arc", "state": {"focused": True}}})
        self.assertEqual(inst.keys, [])

    def test_fails_after_max_presses(self):
        inst = FakeA11yInstance(["Arc", "Bear"])
        with self.assertRaisesRegex(probe.StepFailed, "within 5 press"):
            self.run_step(inst, {"key_until": {"role": "Switch", "text": "Vivaldi", "state": {"focused": True}},
                                 "max_presses": 5, "key": "shift-tab"})
        self.assertEqual(inst.keys, ["shift-tab"] * 5)

    def test_a_refused_key_is_a_failure(self):
        inst = FakeA11yInstance(["Arc", "Bear"], fail_key=True)
        with self.assertRaisesRegex(probe.StepFailed, "returned an error"):
            self.run_step(inst, {"key_until": {"role": "Switch", "text": "Bear", "state": {"focused": True}}})

    def test_validation(self):
        for step in ({"key_until": {"sequence": {"role": "ListItem", "texts": ["a"]}}},
                     {"key_until": {"role": "Switch", "nth": 0}},
                     {"key_until": {"text": "x"}, "max_presses": 0},
                     {"key_until": {"text": "x"}, "key": ""},
                     {"key_until": {}}):
            with self.subTest(step=step), self.assertRaises(probe.ScenarioError):
                _load({"steps": [step]})


class SaveAndProcess(unittest.TestCase):
    class Inst:
        root = data_dir = fixtures_dir = Path("/nonexistent")
        name = "t"

        def __init__(self):
            self.wid = 41
            self.proc = None

        def request(self, verb, args):
            return {"ok": True, "result": {"window_id": self.wid}}

    def test_saved_value_is_compared_later(self):
        inst = self.Inst()
        ctx = probe.RunContext(inst, {})
        out = probe.run_call(ctx, {"call": "window_id", "save": {"wid": "window_id"}})
        self.assertIn("saved", out)
        probe.run_call(ctx, {"call": "window_id", "expect": {"window_id": {"saved": "wid"}}})
        inst.wid = 42
        with self.assertRaises(probe.StepFailed):
            probe.run_call(ctx, {"call": "window_id", "expect": {"window_id": {"saved": "wid"}}})

    def test_saving_a_missing_path_fails(self):
        with self.assertRaisesRegex(probe.StepFailed, "nothing to save"):
            probe.run_call(probe.RunContext(self.Inst(), {}), {"call": "window_id", "save": {"x": "nope"}})

    def test_unresolved_saved_never_matches(self):
        self.assertFalse(probe.match_value(1, {"saved": "x"}))

    def test_saved_must_be_saved_earlier(self):
        with self.assertRaisesRegex(probe.ScenarioError, "not saved by an earlier step"):
            _load({"steps": [{"call": "window_id", "expect": {"window_id": {"saved": "wid"}}},
                             {"call": "window_id", "save": {"wid": "window_id"}}]})
        _load({"steps": [{"call": "window_id", "save": {"wid": "window_id"}},
                         {"call": "window_id", "expect": {"window_id": {"saved": "wid"}}}]})
        for bad in ({"call": "state", "save": {}}, {"call": "state", "save": {"a": 1}},
                    {"call": "state", "expect_error": True, "save": {"a": "b"}},
                    {"call": "state", "expect": {"a": {"saved": "x", "eq": 1}}}):
            with self.subTest(bad=bad), self.assertRaises(probe.ScenarioError):
                _load({"steps": [bad]})

    def test_perf_doc_compare_needs_every_measurement_saved_first(self):
        with self.assertRaisesRegex(probe.ScenarioError, "perf_doc compare needs"):
            _load({"steps": [{"release_budget": True, "cpu_pct_lt": 0.5, "footprint_mb_lt": 80, "binary_mb_lt": 20},
                             {"perf_doc": "x.md", "compare": True}]})
        _load({"steps": [
            {"release_budget": True, "cpu_pct_lt": 0.5, "footprint_mb_lt": 80, "binary_mb_lt": 20},
            {"call": "metrics", "save": {"hotkey_p50_ms": "a", "hotkey_p99_ms": "b"}},
            {"perf_doc": "x.md", "compare": True, "make_target": "budget"}]})

    def test_process_unchanged(self):
        class Proc:
            def __init__(self, pid, rc=None):
                self.pid, self.returncode = pid, rc

            def poll(self):
                return self.returncode

        inst = self.Inst()
        inst.proc = Proc(100)
        ctx = probe.RunContext(inst, {})
        self.assertIn("pid 100", probe.run_process(ctx, {"process": "unchanged"}))
        inst.proc = Proc(101)
        with self.assertRaises(probe.StepFailed):
            probe.run_process(ctx, {"process": "unchanged"})
        inst.proc = Proc(100, rc=0)
        with self.assertRaisesRegex(probe.StepFailed, "exited"):
            probe.run_process(ctx, {"process": "unchanged"})
        with self.assertRaises(probe.ScenarioError):
            _load({"steps": [{"process": "same"}]})


class SeedGraph(unittest.TestCase):
    """seed_graph writes what ubilink-core's import_json writes, in core's schema."""

    def test_schema_matches_ubilink_core(self):
        src = (probe.REPO_ROOT / "src-tauri" / "src" / "graph.rs").read_text()
        m = re.search(r"fn create_base_tables.*?r#\"(.*?)\"#", src, re.S)
        self.assertIsNotNone(m, "create_base_tables's SQL not found in graph.rs")

        def norm(sql):
            sql = re.sub(r"--[^\n]*", "", sql)
            return " ".join(sql.split())

        self.assertEqual(norm(m.group(1)), norm(probe.CORE_GRAPH_SCHEMA))
        steps = re.search(r"const MIGRATIONS: &\[MigrationStep\] = &\[(.*?)\];", src, re.S)
        self.assertEqual(len([s for s in steps.group(1).split(",") if s.strip()]), probe.CORE_GRAPH_VERSION)

    def test_seed_merge_replace_and_list_order(self):
        with tempfile.TemporaryDirectory() as d:
            db = Path(d) / "data" / probe.GRAPH_DB_NAME
            fx = probe.FIXTURES_DIR
            self.assertEqual(probe.seed_graph(db, fx / "graph-backlinks.json"), (5, 3))
            links, n = probe.read_graph(db=db)
            self.assertEqual((len(links), n), (3, 5))
            conn = sqlite3.connect(db)
            try:
                self.assertEqual(conn.execute("PRAGMA user_version").fetchone()[0], probe.CORE_GRAPH_VERSION)
                # ubilink-app's list_links order (updated_at DESC, rowid DESC), which the
                # browser scenarios' row numbers assume.
                order = [r[0] for r in conn.execute(
                    "SELECT name FROM resources ORDER BY updated_at DESC, rowid DESC")]
                self.assertEqual(order[:3], ["Decoy Unrelated", "Gamma Doc", "Beta Post"])
                lid = conn.execute("SELECT id FROM links LIMIT 1").fetchone()[0]
                self.assertIn("\x1f", lid)
            finally:
                conn.close()
            self.assertEqual(probe.seed_graph(db, fx / "graph-backlinks-newer.json"), (6, 4))
            links, n = probe.read_graph(db=db)
            self.assertEqual((len(links), n), (4, 6), "merge: upsert by uri, links idempotent")
            probe.seed_graph(db, fx / "graph-empty.json", replace=True)
            self.assertEqual(probe.read_graph(db=db), (set(), 0))

    def test_bad_inputs(self):
        with tempfile.TemporaryDirectory() as d:
            with self.assertRaises(probe.ScenarioError):
                probe.seed_graph(Path(d) / "g.sqlite", Path(d) / "missing.json")
            bad = Path(d) / "v1.json"
            bad.write_text(json.dumps({"schema": "ubilink.graph.v1", "resources": [], "links": []}))
            with self.assertRaises(probe.ScenarioError):
                probe.seed_graph(Path(d) / "g.sqlite", bad)
        for doc in ({"steps": [{"seed_graph": ""}]}, {"steps": [{"seed_graph": "g.json", "replace": "y"}]},
                    {"steps": [{"restart": True}], "setup": {"graph": [""]}}):
            with self.subTest(doc=doc), self.assertRaises(probe.ScenarioError):
                _load(doc)


class PixelRegionsAndBestMatch(unittest.TestCase):
    def test_state_region(self):
        st = {"view": {"theme_samples": {"surface": {"x": 4, "y": 8, "w": 20, "h": 10}}}}
        self.assertEqual(probe.state_region(st, "view.theme_samples.surface")["w"], 20)
        for bad in ({"view": None}, {"view": {"theme_samples": {"surface": {"x": 1}}}},
                    {"view": {"theme_samples": {"surface": {"x": 1, "y": 1, "w": 0}}}}):
            with self.subTest(bad=bad), self.assertRaises(probe.StepFailed):
                probe.state_region(bad, "view.theme_samples.surface")

    def test_best_pixel_finds_the_glyph_core(self):
        bg, text = (250, 250, 252), (30, 30, 30)
        edges = [(140, 140, 141), (200, 200, 201)]
        de, rgb = probe.best_pixel(text, [bg] * 50 + edges + [text])
        self.assertLess(de, 0.5)
        self.assertEqual(tuple(rgb), text)
        de, _ = probe.best_pixel(text, [bg] * 50 + edges)
        self.assertGreater(de, 3)

    def test_validation(self):
        _load({"steps": [{"pixel": "view.theme_samples.text", "color": "#1e1e1e", "match": "best"}]})
        for bad in ({"pixel": "", "color": "#000000"}, {"pixel": "a.b", "color": "#000000", "match": "median"}):
            with self.subTest(bad=bad), self.assertRaises(probe.ScenarioError):
                _load({"steps": [bad]})


class ReleaseBudgetParsing(unittest.TestCase):
    def test_cputime(self):
        self.assertAlmostEqual(probe.parse_cputime("  0:00.12\n"), 0.12)
        self.assertAlmostEqual(probe.parse_cputime("1:02.50"), 62.5)
        self.assertAlmostEqual(probe.parse_cputime("1:00:01.00"), 3601.0)
        self.assertAlmostEqual(probe.parse_cputime("2-00:00:01"), 2 * 86400 + 1)

    def test_footprint(self):
        self.assertAlmostEqual(probe.parse_footprint("Foo\nPhysical footprint:         41.3M\nPhysical footprint (peak):  50M\n"), 41.3)
        self.assertAlmostEqual(probe.parse_footprint("Physical footprint:         2080K\n"), 2080 / 1024)
        self.assertAlmostEqual(probe.parse_footprint("Physical footprint:   1.5G\n"), 1536.0)
        with self.assertRaises(probe.ProbeError):
            probe.parse_footprint("no such line")

    def test_validation_needs_every_budget(self):
        _load({"steps": [{"release_budget": True, "cpu_pct_lt": 0.5, "footprint_mb_lt": 80, "binary_mb_lt": 20}]})
        for bad in ({"release_budget": True, "cpu_pct_lt": 0.5, "footprint_mb_lt": 80},
                    {"release_budget": "yes", "cpu_pct_lt": 0.5, "footprint_mb_lt": 80, "binary_mb_lt": 20},
                    {"release_budget": True, "cpu_pct_lt": 0, "footprint_mb_lt": 80, "binary_mb_lt": 20}):
            with self.subTest(bad=bad), self.assertRaises(probe.ScenarioError):
                _load({"steps": [bad]})


GOOD_PERF = textwrap.dedent("""\
    # Performance

    | Metric | Budget | Measured |
    |---|---|---|
    | idle_cpu_percent | < 0.5 | 0.12 |
    | physical_footprint_mb | < 80 | 41.3 |
    | stripped_binary_mb | < 20 | 12.3 |
    | hotkey_p50_ms | < 80 | 31 |
    | hotkey_p99_ms | < 200 | 64 |
    """)


class PerfDoc(unittest.TestCase):
    def run_doc(self, text, saved=None, compare=False):
        with tempfile.TemporaryDirectory() as d:
            p = Path(d) / "PERFORMANCE.md"
            if text is not None:
                p.write_text(text)
            ctx = probe.RunContext(SaveAndProcess.Inst(), {})
            ctx.saved = dict(saved or {})
            return probe.run_perf_doc(ctx, {"perf_doc": str(p), "compare": compare})

    def test_good_doc_passes(self):
        self.assertIn("5 rows within budget", self.run_doc(GOOD_PERF))

    def test_missing_doc_row_budget_or_over_budget_fails(self):
        with self.assertRaisesRegex(probe.StepFailed, "PERFORMANCE.md"):
            self.run_doc(None)
        with self.assertRaisesRegex(probe.StepFailed, "rows"):
            self.run_doc(GOOD_PERF.replace("| hotkey_p99_ms | < 200 | 64 |\n", ""))
        with self.assertRaisesRegex(probe.StepFailed, "budget"):
            self.run_doc(GOOD_PERF.replace("< 200", "< 300"))
        with self.assertRaisesRegex(probe.StepFailed, "over budget"):
            self.run_doc(GOOD_PERF.replace("| 41.3 |", "| 81 |"))
        with self.assertRaisesRegex(probe.StepFailed, "not a number"):
            self.run_doc(GOOD_PERF.replace("| 41.3 |", "| ~41 MB |"))
        with self.assertRaisesRegex(probe.StepFailed, "two idle_cpu_percent rows"):
            self.run_doc(GOOD_PERF + "| idle_cpu_percent | < 0.5 | 0.2 |\n")

    def test_compare_within_tolerance(self):
        near = {"idle_cpu_percent": 0.3, "physical_footprint_mb": 47.0, "stripped_binary_mb": 12.9,
                "hotkey_p50_ms": 45.0, "hotkey_p99_ms": 90.0}
        self.assertIn("matching", self.run_doc(GOOD_PERF, near, compare=True))
        far = dict(near, physical_footprint_mb=60.0)
        with self.assertRaisesRegex(probe.StepFailed, "disagrees"):
            self.run_doc(GOOD_PERF, far, compare=True)
        far = dict(near, stripped_binary_mb=14.0)
        with self.assertRaisesRegex(probe.StepFailed, "disagrees"):
            self.run_doc(GOOD_PERF, far, compare=True)

    def test_tolerance_is_the_larger_of_absolute_and_relative(self):
        self.assertEqual(probe.perf_tolerance("hotkey_p99_ms", 40), 40.0)
        self.assertEqual(probe.perf_tolerance("hotkey_p99_ms", 150), 75.0)
        self.assertEqual(probe.perf_tolerance("idle_cpu_percent", 0.1), 0.25)


class ListCommand(unittest.TestCase):
    def test_manifest_names_the_15_phase1_scenarios_and_each_is_valid(self):
        rows = probe.load_manifest()
        names = [n for n, _ in rows]
        self.assertEqual(names, [
            "capture-view", "copy-link", "backlinks", "link-browser", "link-browser-filter", "browser-select",
            "browser-drill", "link-marking", "onboarding", "settings-adapters", "graph-roundtrip", "graph-reset",
            "launch-at-login", "theming", "performance"])
        self.assertEqual([r for _, r in rows], [f"R{i}" for i in range(1, 16)])
        for name, req in rows:
            with self.subTest(name=name):
                sc = probe.load_scenario(name)
                self.assertRegex(sc["requirement"], rf"^{req}\b")

    def test_scenarios_use_only_probes_fixtures_and_placeholders(self):
        # Phase 1's guard diffs only desktop-gpui/probes and scripts/probe.py, so a
        # scenario must not read a file anywhere else, except PERFORMANCE.md, which
        # is R15's product output under judgment, through {repo}.
        for name, _ in probe.load_manifest():
            text = (probe.PROBES_DIR / f"{name}.json").read_text()
            for m in re.finditer(r"\{repo\}([^\"]*)", text):
                self.assertEqual(m.group(1), "/docs/PERFORMANCE.md", name)
            sc = probe.load_scenario(name)
            for step in sc["steps"]:
                for key in ("seed_graph",):
                    if key in step:
                        self.assertTrue((probe.FIXTURES_DIR / step[key]).is_file(), step[key])
            for g in ([sc.get("setup", {}).get("graph")] if isinstance(sc.get("setup", {}).get("graph"), str)
                      else sc.get("setup", {}).get("graph", [])):
                self.assertTrue((probe.FIXTURES_DIR / g).is_file(), g)
            for p in (sc.get("setup", {}).get("fixtures") or {}).values():
                if not p.startswith("{"):
                    self.assertTrue((probe.FIXTURES_DIR / p).is_file(), p)

    def test_verdict(self):
        all_fail = [(f"s{i}", f"R{i}", 1) for i in range(3)]
        self.assertEqual(probe.list_verdict(all_fail, 3, 3), [])
        self.assertTrue(probe.list_verdict(all_fail, 4, None), "wrong count")
        one_error = all_fail[:2] + [("s2", "R2", 2)]
        self.assertTrue(any("exited 2" in p for p in probe.list_verdict(one_error, 3, 3)))
        blocked = all_fail[:2] + [("s2", "R2", 3)]
        problems = probe.list_verdict(blocked, 3, 2)
        self.assertTrue(any("ENV-BLOCKED" in p for p in problems), "a 3 counts against the expectation")
        mixed = [("a", "R1", 0), ("b", "R2", 1)]
        self.assertEqual(probe.list_verdict(mixed, 2, 1), [])
        self.assertTrue(probe.list_verdict([("a", "R1", None)], 1, 1), "not run")

    def test_bad_manifest(self):
        with tempfile.TemporaryDirectory() as d:
            for doc in ({}, {"scenarios": []}, {"scenarios": [{"name": "a"}]},
                        {"scenarios": [{"name": "a", "requirement": "R1"}, {"name": "a", "requirement": "R2"}]}):
                p = Path(d) / "m.json"
                p.write_text(json.dumps(doc))
                with self.subTest(doc=doc), self.assertRaises(probe.ScenarioError):
                    probe.load_manifest(p)

    def test_first_problem_line(self):
        out = "PASS  x\nFAIL  s [2/9] see ...\n      detail\nFAIL: run s\n"
        self.assertEqual(probe.first_problem_line(out), "FAIL  s [2/9] see ...")


class LoginItemFixtures(unittest.TestCase):
    def test_validator(self):
        ok = {"status": "not_registered", "on_register": "not_found",
              "explanation": "macOS cannot find this copy; move it to Applications and retry."}
        self.assertEqual(probe._validate_login_item(ok, "not_found"), [])
        for bad in (dict(ok, status="enabled"), dict(ok, explanation="nope"), dict(ok, calls=[]),
                    {"status": "not_registered", "on_register": "not_found"}):
            with self.subTest(bad=bad):
                self.assertTrue(probe._validate_login_item(bad, "not_found"))
        self.assertTrue(probe._validate_login_item(
            {"status": "not_registered", "on_register": "enabled", "explanation": "x y z a b c"}, "enabled"))

    def test_kind_is_one_the_app_accepts(self):
        src = (probe.REPO_ROOT / "desktop-gpui" / "src" / "automation" / "protocol.rs").read_text()
        self.assertIn('(Fixture::LoginItem, "login_item")', src)


class CaptureSubjectFixture(unittest.TestCase):
    def test_subject_capture_must_be_the_backlinks_subject(self):
        good = {"name": "Subject Note", "uri": probe._BACKLINKS_SUBJECT_URI, "app": "Safari"}
        self.assertEqual(probe._validate_capture_subject(good), [])
        self.assertTrue(probe._validate_capture_subject(dict(good, uri="https://example.com/")))
        self.assertTrue(probe._BACKLINKS_SUBJECT_URI.startswith("https://"),
                        "the core's default capture allowlist must admit the subject")


if __name__ == "__main__":
    unittest.main()
