#!/usr/bin/env python3
"""Self-tests for ubilink_instance.py's session-lease rules (no app, no GUI).

    python3 desktop-gpui/.claude/skills/verify-ubilink/scripts/test_ubilink_instance.py

The decision table (`lease_decision`) is tested as a pure function. Reclaim is
exercised end to end against stand-in "instances": a copy of /bin/sleep runs under a
temp path recorded as the claim's binary, so `is_ours` really matches it and the
reclaim really signals it. The lease, its lock and the run dir live in temp
directories (UBILINK_PROBE_CACHE, VERIFY_EVIDENCE_DIR), never the real ones.
"""
import importlib.util
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

TMP = Path(tempfile.mkdtemp(prefix="ubinst-test-"))
os.environ["UBILINK_PROBE_CACHE"] = str(TMP / "cache")
os.environ["VERIFY_EVIDENCE_DIR"] = str(TMP / "root" / ".verify")

HERE = Path(__file__).resolve().parent
_spec = importlib.util.spec_from_file_location("ubilink_instance", HERE / "ubilink_instance.py")
ui = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(ui)

ME = {"run_root": "/runs/x/.verify-run", "verifier": "v2"}
NOW = 10_000.0
DEAD = lambda pid: False  # noqa: E731
ALIVE = lambda pid: bool(pid)  # noqa: E731


def lease(**kw):
    doc = {"run_root": "/runs/x/.verify-run", "verifier": "v1", "instances": ["a"],
           "launcher_pid": None, "active_pid": None, "last_activity": NOW - 30}
    doc.update(kw)
    return doc


class Decision(unittest.TestCase):
    def test_no_lease_is_taken(self):
        self.assertEqual(ui.lease_decision(None, ME, NOW, [], DEAD), "take")

    def test_own_lease_is_joined_even_if_idle(self):
        self.assertEqual(ui.lease_decision(lease(verifier="v2", last_activity=0), ME, NOW,
                                           ["a"], DEAD), "join")

    def test_same_verifier_other_run_dir_is_not_ours(self):
        doc = lease(verifier="v2", run_root="/runs/other/.verify-run", last_activity=NOW)
        self.assertEqual(ui.lease_decision(doc, ME, NOW, ["a"], DEAD), "wait")

    def test_recent_activity_waits(self):
        self.assertEqual(ui.lease_decision(lease(), ME, NOW, ["a"], DEAD), "wait")

    def test_idle_past_limit_with_live_instances_is_reclaimed(self):
        doc = lease(last_activity=NOW - ui.LEASE_IDLE_S - 1)
        self.assertEqual(ui.lease_decision(doc, ME, NOW, ["a"], DEAD), "reclaim")

    def test_exactly_at_limit_still_waits(self):
        doc = lease(last_activity=NOW - ui.LEASE_IDLE_S)
        self.assertEqual(ui.lease_decision(doc, ME, NOW, ["a"], DEAD), "wait")

    def test_running_command_is_never_reclaimed_however_idle(self):
        for key in ("launcher_pid", "active_pid"):
            doc = lease(last_activity=0, **{key: 4242})
            self.assertEqual(ui.lease_decision(doc, ME, NOW, ["a"], ALIVE), "wait", key)

    def test_dead_command_pid_does_not_protect(self):
        doc = lease(last_activity=0, active_pid=4242)
        self.assertEqual(ui.lease_decision(doc, ME, NOW, ["a"], DEAD), "reclaim")

    def test_no_live_instances_is_stale_not_reclaim(self):
        self.assertEqual(ui.lease_decision(lease(), ME, NOW, [], DEAD), "stale")

    def test_lease_without_heartbeat_counts_as_idle(self):
        doc = lease()
        del doc["last_activity"]
        self.assertEqual(ui.lease_decision(doc, ME, NOW, ["a"], DEAD), "reclaim")

    def test_limit_is_shorter_than_the_launch_wait(self):
        # A re-dispatched verifier must be able to reclaim inside its own wait.
        self.assertLess(ui.LEASE_IDLE_S, ui.LEASE_TIMEOUT_S)


class Reclaim(unittest.TestCase):
    def setUp(self):
        self.procs = []
        shutil.rmtree(TMP / "cache", ignore_errors=True)
        shutil.rmtree(TMP / "root", ignore_errors=True)
        (TMP / "cache").mkdir(parents=True)
        self.root = ui.run_root()
        self.root.mkdir(parents=True)

    def tearDown(self):
        for p in self.procs:
            if p.poll() is None:
                p.kill()
                p.wait()

    def fake_instance(self, name, verifier="v1"):
        """A claim whose 'binary' is a copy of sleep that is really running."""
        d = self.root / name
        d.mkdir()
        binary = d / "ubilink-next-automation"
        shutil.copy("/bin/sleep", binary)
        # A copied system binary loses its valid signature and macOS kills it on exec;
        # an ad-hoc signature lets it run under the new path.
        subprocess.run(["codesign", "-s", "-", "--force", str(binary)], check=True,
                       capture_output=True)
        proc = subprocess.Popen([str(binary), "300"])
        self.procs.append(proc)
        deadline = time.monotonic() + 5
        while ui.pid_command(proc.pid) != str(binary) and time.monotonic() < deadline:
            time.sleep(0.05)
        doc = {"status": "running", "instance": name, "verifier": verifier,
               "pid": proc.pid, "binary": str(binary), "socket": None, "si_socket": None,
               "root": None}
        (d / "instance.json").write_text(json.dumps(doc))
        return proc, doc

    def write_lease(self, **kw):
        doc = {"run_root": str(self.root), "verifier": "v1", "instances": ["va"],
               "launcher_pid": None, "active_pid": None, "last_activity": time.time()}
        doc.update(kw)
        ui.LEASE.parent.mkdir(parents=True, exist_ok=True)
        ui.LEASE.write_text(json.dumps(doc))

    def test_idle_lease_is_reclaimed_and_the_holder_is_told(self):
        proc, claim = self.fake_instance("va")
        self.assertTrue(ui.is_ours(claim))
        self.write_lease(last_activity=time.time() - ui.LEASE_IDLE_S - 5)
        ui.lease_acquire("v2", "vb", timeout=0)
        proc.wait(timeout=10)  # the reclaim signalled the holder's instance
        lease = json.loads(ui.LEASE.read_text())
        self.assertEqual((lease["verifier"], lease["instances"]), ("v2", ["vb"]))
        self.assertFalse((self.root / "va").exists(), "the holder's claim is removed")
        recs = list(ui.reclaim_dir(self.root).glob("*.json"))
        self.assertEqual(len(recs), 1)
        rec = json.loads(recs[0].read_text())
        self.assertEqual((rec["from_verifier"], rec["by_verifier"], rec["instances"]),
                         ("v1", "v2", ["va"]))
        self.assertEqual(rec["stopped"][0]["pid"], proc.pid)
        # The returning holder gets exit 3 with the reason, on every entry point.
        with self.assertRaises(ui.Blocked) as cm:
            ui.load_state("va", "v1")
        self.assertIn("lease reclaimed after idle", str(cm.exception))
        self.assertEqual(cm.exception.code, 3)
        with self.assertRaises(ui.Blocked) as cm:
            ui.lease_touch(claim, active=True)
        self.assertIn("lease reclaimed after idle", str(cm.exception))

    def test_active_lease_is_not_reclaimed(self):
        proc, _ = self.fake_instance("va")
        self.write_lease(last_activity=time.time())
        with self.assertRaises(ui.Blocked):
            ui.lease_acquire("v2", "vb", timeout=0)
        self.assertIsNone(proc.poll(), "an active holder's instance keeps running")
        self.assertEqual(json.loads(ui.LEASE.read_text())["verifier"], "v1")

    def test_running_command_protects_an_idle_lease(self):
        proc, _ = self.fake_instance("va")
        self.write_lease(last_activity=0, active_pid=os.getpid())
        with self.assertRaises(ui.Blocked):
            ui.lease_acquire("v2", "vb", timeout=0)
        self.assertIsNone(proc.poll())

    def test_reused_pid_is_not_signalled(self):
        proc, claim = self.fake_instance("va")
        claim["binary"] = "/some/other/binary"  # the pid no longer runs our binary
        (self.root / "va" / "instance.json").write_text(json.dumps(claim))
        self.write_lease(last_activity=0)
        ui.lease_acquire("v2", "vb", timeout=0)  # nothing alive of ours: stale, replaced
        self.assertIsNone(proc.poll(), "a process that is not our instance is never killed")
        self.assertEqual(json.loads(ui.LEASE.read_text())["verifier"], "v2")

    def test_heartbeat_refreshes_and_marks_active(self):
        _, claim = self.fake_instance("va")
        self.write_lease(last_activity=0)
        ui.lease_touch(claim, active=True)
        lease = json.loads(ui.LEASE.read_text())
        self.assertEqual(lease["active_pid"], os.getpid())
        self.assertGreater(lease["last_activity"], time.time() - 5)
        ui.lease_touch(claim, active=False)
        self.assertIsNone(json.loads(ui.LEASE.read_text())["active_pid"])


class FakeRemote:
    """Enough of ubilink_instance.Remote for attempt_call's reset_panel."""

    def __init__(self, exc):
        self.exc, self.log, self.label = exc, [], "va"
        self.doc = {"pid": 1, "instance": "va"}
        self.proc = type("P", (), {"pid": 1})()
        self.root = self.data_dir = self.fixtures_dir = TMP
        self.name = "vf-va"

    def call(self, verb, **args):
        raise self.exc


class ResetPanelExits(unittest.TestCase):
    """Ruling R-x: a reset_panel error never escapes as a traceback."""

    def test_app_refusal_is_a_product_fail(self):
        outcome, _ = ui.attempt_call(FakeRemote(ui.probe.CallError("no panel")), [], None, 1, 1)
        self.assertEqual(outcome, "fail")

    def test_socket_trouble_is_a_harness_error(self):
        with self.assertRaises(ui.probe.ProbeError):
            ui.attempt_call(FakeRemote(ui.probe.ProbeError("socket closed")), [], None, 1, 1)

    def test_main_maps_every_exception_to_an_exit_code(self):
        cases = [(ui.probe.ProbeError("x"), 2), (ui.probe.EnvBlocked("x"), 3),
                 (ui.probe.ScenarioError("x"), 2), (ui.probe.CallError("x"), 1),
                 (ui.Blocked("x"), 3), (ui.HarnessError("x"), 2), (RuntimeError("x"), 2)]
        real = ui.cmd_list
        try:
            for exc, code in cases:
                def raiser(_a, exc=exc):
                    raise exc
                ui.cmd_list = raiser
                self.assertEqual(ui.main(["list"]), code, repr(exc))
        finally:
            ui.cmd_list = real


if __name__ == "__main__":
    try:
        unittest.main(verbosity=2)
    finally:
        shutil.rmtree(TMP, ignore_errors=True)
