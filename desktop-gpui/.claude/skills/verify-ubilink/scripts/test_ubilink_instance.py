#!/usr/bin/env python3
"""Self-tests for ubilink_instance.py's session-lease and restart rules (no app, no GUI).

    python3 desktop-gpui/.claude/skills/verify-ubilink/scripts/test_ubilink_instance.py

The decision table (`lease_decision`) is tested as a pure function. Reclaim is
exercised end to end against stand-in "instances": a copy of /bin/sleep runs under a
temp path recorded as the claim's binary, so `is_ours` really matches it and the
reclaim really signals it. The lease, its lock and the run dir live in temp
directories (UBILINK_PROBE_CACHE, VERIFY_EVIDENCE_DIR), never the real ones.

Restart is exercised end to end against a stand-in app (FAKE_APP below): a script that
serves the automation socket at the path the real app uses, answers `state` with its
UBILINK_DATA_DIR and logs every `load_fixture` into that data dir, so the test sees the
data dir survive, the fixtures replayed and the new pid recorded.
"""
import contextlib
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


FAKE_APP = r"""#!/usr/bin/env python3
import json, os, socket, sys
tmp = os.environ["TMPDIR"]
path = os.path.join(tmp, "ubilink-%d" % os.getuid(), os.environ["UBILINK_INSTANCE"], "auto.sock")
os.makedirs(os.path.dirname(path), mode=0o700, exist_ok=True)
data = os.environ["UBILINK_DATA_DIR"]
assert os.environ.get("UBILINK_SKIP_HOTKEYS") == "1"
with open(os.path.join(data, "launches.log"), "a") as f:
    f.write("%d\n" % os.getpid())
if os.path.exists(path):
    os.unlink(path)
srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
srv.bind(path)
srv.listen(4)
while True:
    conn, _ = srv.accept()
    r = conn.makefile("rb")
    for line in r:
        req = json.loads(line)
        if req["verb"] == "load_fixture":
            with open(os.path.join(data, "fixtures.log"), "a") as f:
                f.write(json.dumps(req["args"], sort_keys=True) + "\n")
            res = {"ok": True, "result": {req["args"]["kind"]: req["args"]["path"]}}
        elif req["verb"] == "state":
            res = {"ok": True, "result": {"data_dir": data, "shell": {"hotkeys_skipped": True}}}
        else:
            res = {"ok": False, "error": "unknown verb"}
        conn.sendall((json.dumps(res) + "\n").encode())
    conn.close()
"""


def alive_not_zombie(doc):
    pid = (doc or {}).get("pid")
    if not pid:
        return False
    r = subprocess.run(["ps", "-p", str(pid), "-o", "stat="], capture_output=True, text=True)
    return bool(r.stdout.strip()) and "Z" not in r.stdout


class RestartRules(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="ubrst-", dir=TMP))
        self.root = self.tmp / "ubprobe-vf-x-abc"
        (self.root / "data").mkdir(parents=True)
        self.doc = {"instance": "x", "root": str(self.root),
                    "data_dir": str(self.root / "data"), "binary": "/b", "binary_sha256": "s"}

    def test_restartable(self):
        self.assertIsNone(ui.restart_problem(self.doc, "s"))

    def test_changed_binary_refused(self):
        self.assertIn("changed since launch", ui.restart_problem(self.doc, "other"))
        self.assertIn("changed since launch", ui.restart_problem(self.doc, None))

    def test_missing_data_dir_refused(self):
        shutil.rmtree(self.root / "data")
        self.assertIn("data dir", ui.restart_problem(self.doc, "s"))

    def test_non_probe_root_refused(self):
        self.doc["root"] = "/Users/someone"
        self.assertIn("no probe temp root", ui.restart_problem(self.doc, "s"))


class RestartEndToEnd(unittest.TestCase):
    """cmd_restart against the stand-in app: same temp root, env and binary."""

    def setUp(self):
        shutil.rmtree(TMP / "cache", ignore_errors=True)
        shutil.rmtree(TMP / "root", ignore_errors=True)
        (TMP / "cache").mkdir(parents=True)
        self.run_root = ui.run_root()
        self.run_root.mkdir(parents=True)
        self.real_is_ours = ui.is_ours
        ui.is_ours = alive_not_zombie  # the stand-in is a script: ps shows its interpreter
        self.binary = TMP / "fake-ubilink-next"
        self.binary.write_text(FAKE_APP)
        self.binary.chmod(0o755)
        self.inst = ui.probe.Instance("vr", str(self.binary))
        self.inst.name = "vf-vr-test"
        self.inst.start(timeout=20)  # probe.py's own launch: env, temp root, socket wait
        self.inst._reader.close()  # detach as `launch` does (the stand-in serves one
        self.inst._sock.close()    # connection at a time)
        self.addCleanup(self.cleanup)
        self.doc = {"status": "running", "instance": "vr", "verifier": "v1",
                    "app_instance": self.inst.name, "pid": self.inst.proc.pid,
                    "binary": str(self.binary),
                    "binary_sha256": ui.probe._sha256_file(self.binary),
                    "root": str(self.inst.root), "data_dir": str(self.inst.data_dir),
                    "fixtures": str(self.inst.fixtures_dir),
                    "socket": str(self.inst.socket_path),
                    "si_socket": str(self.inst.si_socket_path), "log": str(self.inst.log_path)}
        (self.run_root / "vr").mkdir()
        ui.write_claim(self.doc)
        ui.LEASE.write_text(json.dumps({"run_root": str(self.run_root), "verifier": "v1",
                                        "instances": ["vr"], "launcher_pid": None,
                                        "active_pid": None, "last_activity": time.time()}))

    def cleanup(self):
        ui.is_ours = self.real_is_ours
        claim = ui.read_json(self.run_root / "vr" / "instance.json") or self.doc
        for pid in {self.doc["pid"], claim.get("pid")}:
            if pid:
                with contextlib.suppress(OSError):
                    os.kill(pid, 9)
        self.inst.stop()

    def args(self, verifier="v1"):
        return type("A", (), {"instance": "vr", "verifier": verifier, "timeout": 20})()

    def test_restart_keeps_data_replays_fixtures_records_new_pid(self):
        (self.inst.data_dir / "user-state.json").write_text('{"kept": true}')
        r = ui.Remote(self.doc)  # a fixture loaded through `call`'s path is remembered
        try:
            r.call("load_fixture", kind="capture", path="/fx/capture-ok.json")
            r.call("load_fixture", kind="login_item", path="/fx/login-1.json")
            r.call("load_fixture", kind="capture", path="/fx/capture-failed.json")
        finally:
            r.close()
        claim = ui.read_json(self.run_root / "vr" / "instance.json")
        self.assertEqual(claim["fixtures_loaded"], {"capture": "/fx/capture-failed.json",
                                                    "login_item": "/fx/login-1.json"})
        old_pid = self.doc["pid"]
        self.assertEqual(ui.cmd_restart(self.args()), 0)
        claim = ui.read_json(self.run_root / "vr" / "instance.json")
        self.assertNotEqual(claim["pid"], old_pid)
        self.assertEqual(claim["restarts"], 1)
        self.assertFalse(alive_not_zombie({"pid": old_pid}), "the old process is gone")
        self.assertTrue(alive_not_zombie(claim), "the new process runs")
        self.assertTrue(Path(claim["root"]).is_dir(), "the temp root is kept")
        self.assertEqual((self.inst.data_dir / "user-state.json").read_text(), '{"kept": true}')
        launches = (self.inst.data_dir / "launches.log").read_text().split()
        self.assertEqual(launches, [str(old_pid), str(claim["pid"])])
        replayed = [json.loads(x) for x in
                    (self.inst.data_dir / "fixtures.log").read_text().splitlines()][3:]
        self.assertEqual(sorted(x["kind"] for x in replayed), ["capture", "login_item"])
        self.assertIn({"kind": "capture", "path": "/fx/capture-failed.json"}, replayed)
        r = ui.Remote(claim)  # and it answers on the same socket path
        try:
            self.assertEqual(r.call("state")["data_dir"], self.doc["data_dir"])
        finally:
            r.close()
        lease = json.loads(ui.LEASE.read_text())
        self.assertIsNone(lease["active_pid"], "restart cleared its heartbeat mark")

    def test_other_verifier_cannot_restart(self):
        with self.assertRaises(ui.Blocked):
            ui.cmd_restart(self.args("v2"))
        self.assertEqual(ui.read_json(self.run_root / "vr" / "instance.json")["pid"],
                         self.doc["pid"])

    def test_changed_binary_exits_2_and_keeps_the_instance(self):
        self.binary.write_text(FAKE_APP + "\n# rebuilt\n")
        self.assertEqual(ui.main(["restart", "--instance", "vr", "--verifier", "v1"]), 2)
        self.assertTrue(alive_not_zombie(self.doc), "nothing was stopped")

    def test_terminate_keeps_root_stop_removes_it(self):
        ui.terminate_process(self.doc)
        self.assertFalse(alive_not_zombie(self.doc))
        self.assertTrue(Path(self.doc["data_dir"]).is_dir())
        self.assertFalse(Path(self.doc["socket"]).exists())
        ui.stop_instance(self.doc)
        self.assertFalse(Path(self.doc["root"]).exists())


if __name__ == "__main__":
    try:
        unittest.main(verbosity=2)
    finally:
        shutil.rmtree(TMP, ignore_errors=True)
