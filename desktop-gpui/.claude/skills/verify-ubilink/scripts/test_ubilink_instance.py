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
if os.path.exists(os.path.join(data, "hang")):  # a start that never serves its socket
    import time
    time.sleep(3600)
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
        elif req["verb"] == "opened":
            res = {"ok": True, "result": []}
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


    def test_failed_relaunch_is_killed_and_the_claim_says_so(self):
        # Review I5: the relaunched process never serves its socket.
        (self.inst.data_dir / "hang").write_text("")
        code = ui.main(["restart", "--instance", "vr", "--verifier", "v1", "--timeout", "2"])
        self.assertEqual(code, 2)
        launches = (self.inst.data_dir / "launches.log").read_text().split()
        self.assertEqual(len(launches), 2, "the relaunch did start a process")
        self.assertFalse(alive_not_zombie({"pid": int(launches[-1])}),
                         "the half-started process is killed, not orphaned")
        claim = ui.read_json(self.run_root / "vr" / "instance.json")
        self.assertEqual((claim["status"], claim["pid"]), ("failed-restart", None))
        self.assertTrue(Path(claim["root"]).is_dir(), "the root stays for stop to clean")
        with self.assertRaises(ui.HarnessError):
            ui.load_state("vr", "v1")  # call/doctor refuse it; stop cleans up
        stop = type("A", (), {"instance": "vr", "verifier": "v1"})()
        self.assertEqual(ui.cmd_stop(stop), 0)
        self.assertFalse(Path(claim["root"]).exists())


SCENARIOS = {
    "t-pass": {"description": "stand-in pass", "requirement": "T1",
               "setup": {"fixtures": {"capture": "capture-ok.json"}},
               "steps": [
                   {"call": "state", "expect": {"data_dir": "{data_dir}"}},
                   {"call": "opened", "expect": {"": {"eq": []}}},
                   {"restart": True},
                   {"process": "unchanged"},
                   {"call": "state", "expect": {"data_dir": "{data_dir}"}}]},
    "t-fail": {"description": "stand-in fail",
               "steps": [{"call": "state"},
                         {"call": "state", "expect": {"data_dir": "/not/this"}},
                         {"call": "opened"}]},
}


class ScenarioCommand(RestartEndToEnd):
    """`scenario` runs probe.py's own run_scenario against the verifier's instance."""

    def setUp(self):
        super().setUp()
        self.probes = TMP / "probes"
        self.probes.mkdir(exist_ok=True)
        for name, sc in SCENARIOS.items():
            (self.probes / ("%s.json" % name)).write_text(json.dumps(sc))
        (self.probes / "t-bad.json").write_text(json.dumps({"steps": [{"nope": 1}]}))
        self.patches = [(ui, "PROBES_DIR", self.probes),
                        (ui, "screen_locked", lambda: False),
                        (ui.probe, "active_displays", lambda: [{"id": 1}]),
                        (ui.probe, "wait_for_display", lambda *a, **k: None),
                        (ui.probe, "_helper", lambda *a, **k: None),
                        (ui.probe, "keep_awake", lambda: None),
                        (ui.probe, "source_revision", lambda repo=None: ("c0ffee", False))]
        self.saved_attrs = [(m, n, getattr(m, n)) for m, n, _ in self.patches]
        for m, n, v in self.patches:
            setattr(m, n, v)
        self.doc.update(commit="c0ffee", dirty=False)
        ui.write_claim(self.doc)
        self.out = TMP / "out"
        shutil.rmtree(self.out, ignore_errors=True)

        self.addCleanup(self.stop_claimed)  # whatever the scenario left running

    def stop_claimed(self):
        claim = ui.read_json(self.run_root / "vr" / "instance.json")
        if claim:
            ui.stop_instance(claim)

    def tearDown(self):
        for m, n, v in self.saved_attrs:
            setattr(m, n, v)

    def run_scenario(self, *names, verifier="v1", instances=1):
        argv = ["scenario", "--instance", "vr", "--verifier", verifier, "--out", str(self.out),
                "--instances", str(instances)]
        for n in names:
            argv += ["--name", n]
        return ui.main(argv)

    def test_pass_with_restart_records_claim_and_artifacts(self):
        before = (self.probes / "t-pass.json").read_bytes()
        self.assertEqual(self.run_scenario("t-pass"), 0)
        self.assertEqual((self.probes / "t-pass.json").read_bytes(), before, "input untouched")
        d = self.out / "t-pass"
        res = json.loads((d / "result.json").read_text())
        self.assertEqual((res["exit"], res["verdict"], res["scenario"]), (0, "PASS", "t-pass"))
        self.assertEqual(res["sha"], "c0ffee")
        self.assertEqual([s["verdict"] for s in res["steps"]], ["PASS"] * 5)
        self.assertEqual([s["index"] for s in res["steps"]], [1, 2, 3, 4, 5])
        self.assertTrue((d / "transcript.txt").read_text().count("PASS") >= 5)
        for f in res["step_records"]:
            self.assertTrue((d / f).is_file(), f)
        rec = json.loads((d / res["step_records"][0]).read_text())
        self.assertEqual(rec["exchanges"][0]["verb"], "state")
        self.assertEqual(sorted(res["artifacts"]), sorted(p.name for p in d.iterdir()))
        # The verifier's own instance: a fresh root, restarted in place, still running,
        # and its claim names the live (post-restart) pid and the replayed fixture.
        claim = ui.read_json(self.run_root / "vr" / "instance.json")
        self.assertNotEqual(claim["root"], self.doc["root"])
        self.assertFalse(Path(self.doc["root"]).exists(), "the pre-scenario root is gone")
        self.assertTrue(alive_not_zombie(claim))
        self.assertEqual(claim["pid"], res["primary_pid_after"])
        launches = (Path(claim["data_dir"]) / "launches.log").read_text().split()
        self.assertEqual(len(launches), 2, "launched fresh, then the restart step")
        self.assertEqual(int(launches[-1]), claim["pid"])
        fixtures = (Path(claim["data_dir"]) / "fixtures.log").read_text().splitlines()
        self.assertEqual(len(fixtures), 2, "setup fixture loaded, then replayed on restart")
        self.assertIn("capture", claim["fixtures_loaded"])

    def test_product_fail_exits_1_and_marks_the_step(self):
        self.assertEqual(self.run_scenario("t-fail"), 1)
        res = json.loads((self.out / "t-fail" / "result.json").read_text())
        self.assertEqual(res["verdict"], "FAIL")
        self.assertEqual([s["verdict"] for s in res["steps"]], ["PASS", "FAIL"])
        self.assertTrue(any("/not/this" in m for m in res["steps"][1]["more"]))

    def test_bad_scenario_and_paths_exit_2_before_touching_the_instance(self):
        self.assertEqual(self.run_scenario("t-bad"), 2)
        self.assertEqual(self.run_scenario("../x"), 2)
        self.assertEqual(self.run_scenario("nonexistent"), 2)
        self.assertEqual(ui.read_json(self.run_root / "vr" / "instance.json")["pid"],
                         self.doc["pid"])

    def test_other_verifier_is_refused(self):
        self.assertEqual(self.run_scenario("t-pass", verifier="v2"), 3)
        self.assertEqual(ui.read_json(self.run_root / "vr" / "instance.json")["pid"],
                         self.doc["pid"])

    def test_stale_build_is_refused(self):
        ui.probe.source_revision = lambda repo=None: ("deadbeef", False)
        self.assertEqual(self.run_scenario("t-pass"), 2)

    def test_two_instances_claims_extra_and_cleans_it(self):
        self.assertEqual(self.run_scenario("t-fail", instances=2), 1)
        res = json.loads((self.out / "t-fail" / "result.json").read_text())
        self.assertEqual(res["extra_instances"], ["vr-2"])
        self.assertFalse((self.run_root / "vr-2").exists(), "the extra claim is removed")
        lease = json.loads(ui.LEASE.read_text())
        self.assertEqual(lease["instances"], ["vr"], "the extra left the lease")
        self.assertEqual({s["instance"] for s in res["steps"]}, {"t-fail#1", "t-fail#2", "t-fail"})

    def test_transcript_parser(self):
        steps = ui.parse_transcript(
            "── x: d\n"
            "PASS  x [1/3] call state — ok\n"
            "RETRY x [2/3] winframe any\n"
            "      focus taken by Mail\n"
            "PASS  x [2/3] winframe any — inside\n"
            "FAIL  x [3/3] see Label\n"
            "      expected: a\n"
            "PASS  x#1 [side-by-side] answers state while 2 run — pid 1\n"
            "ERROR x: harness: boom\n"
            "FAIL: run x (instances=1) — 2 passed\n")
        self.assertEqual([(s["index"], s["verdict"], s["retried"]) for s in steps],
                         [(1, "PASS", False), (2, "PASS", True), (3, "FAIL", False),
                          ("side-by-side:answers state while 2 run", "PASS", False),
                          ("scenario", "ERROR", False)])
        self.assertEqual(steps[2]["more"], ["expected: a"])


if __name__ == "__main__":
    try:
        unittest.main(verbosity=2)
    finally:
        shutil.rmtree(TMP, ignore_errors=True)
