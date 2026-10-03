"""Self-tests for scripts/probe.py's scenario engine (GPUI Phase 0 R8).

Headless and stdlib-only: everything here runs without building or launching the
app, so it covers the parts a live `probe.py run` cannot prove on its own — that a
typo in a scenario is refused instead of passing vacuously, that matching tolerates
truncation without matching everything, that ENV-BLOCKED is not reported as a
product FAIL, and that the desktop lock really excludes a second holder.

    python3 -m unittest scripts/test_probe.py
"""

import contextlib
import io
import json
import os
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


if __name__ == "__main__":
    unittest.main()
