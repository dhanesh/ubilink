#!/usr/bin/env python3
"""ubilink_instance.py: long-lived, named ubilink-next instances for verify-ubilink.

`scripts/probe.py run` launches instances, runs one scenario and tears them down in a
single process. A verifier needs the same instances to outlive one command: launch,
doctor, drive step by step, record evidence, clean up. This harness is that split of
probe.py's own engine. It imports scripts/probe.py and uses its build (`build_binary`,
the warm locked target and per-run binary copy), its isolation (`Instance`: own
UBILINK_INSTANCE automation socket, own UBILINK_INSTANCE_SOCKET, own UBILINK_DATA_DIR,
own fixture copy, UBILINK_SKIP_HOTKEYS=1), its desktop lock and its matchers. It adds
no semantics to probe.py and changes none.

Usage (from anywhere inside the repo; JSON on stdout, build/lock chatter on stderr):
    ubilink_instance.py launch --instance a            # INSTANCE: {...}   exit 0|2|3
    ubilink_instance.py doctor --instance a [--as-args] # read-only checks  exit 0|1|2
    ubilink_instance.py call   --instance a [--out DIR] STEP [STEP ...]     exit 0|1|2|3
    ubilink_instance.py stop   --instance a            # STOPPED: {...}    exit 0|2
    ubilink_instance.py list                           # claimed instances

STEP is `verb` or `verb:JSON-args` for the automation verbs (open_view, key, state,
metrics, trigger_hotkey, resign_key, set_appearance, load_fixture, window_id, a11y,
opened), or one of the harness's checks:
    wait_state:{"dot.path": literal-or-matcher, ...}   poll `state` until every path holds
    see:{"role": "Label", "text": "Capture"}           poll `a11y` until the query holds
    absent:{"text": "Browse links"}                    poll `a11y` until no node matches
    screenshot[:label]                                 screencapture -x -o -l<window_id>
    seed_graph:{"fixture": "graph-backlinks.json", "replace": false}
                                 setup only: probe.py's seed_graph into {data_dir}'s graph
Matchers and a11y queries are probe.py's (desktop-gpui/probes/README.md). Strings may
use {root}, {data_dir}, {fixtures}, {instance}.

Where state lives: <run dir>/<instance>/instance.json, where <run dir> is the sibling
.verify-run/ of the evidence directory (verify_evidence.py's run_dir: $VERIFY_EVIDENCE_DIR
or <git toplevel>/.verify). The claim is O_EXCL, so two owners can never hold one
instance name. This harness never reads, writes or deletes the evidence directory.

Exit codes: 0 ok; 1 the product did not do what a step expects (an {"ok": false} reply,
a wait_state/see/absent that never held) or doctor found a failed check; 2 harness error
(bad input, no such instance, build failure, socket gone); 3 environment-blocked (the
instance name is taken, Screen Recording not granted).
"""
import argparse
import contextlib
import hashlib
import importlib.util
import json
import os
import re
import shutil
import signal
import socket
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
# <repo>/desktop-gpui/.claude/skills/verify-ubilink/scripts -> <repo>
REPO = HERE.parents[4]
PROBE_PY = REPO / "scripts" / "probe.py"
NAME_RE = re.compile(r"^[a-z][a-z0-9-]{0,15}\Z")
VERBS = ("open_view", "key", "state", "metrics", "trigger_hotkey", "resign_key",
         "set_appearance", "load_fixture", "window_id", "a11y", "opened")
CHECKS = ("wait_state", "see", "absent", "screenshot", "seed_graph")
DEFAULT_TIMEOUT_MS = 5000


def _load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    mod = importlib.util.module_from_spec(spec)
    sys.modules[name] = mod
    spec.loader.exec_module(mod)
    return mod


probe = _load("probe", PROBE_PY)
recorder = _load("verify_evidence", HERE / "verify_evidence.py")


class HarnessError(Exception):
    code = 2


class Blocked(HarnessError):
    code = 3


class Failed(Exception):
    """The product did not do what a step expects."""


def err(msg):
    sys.stderr.write("ubilink_instance: %s\n" % msg)


def emit(obj):
    print(json.dumps(obj, sort_keys=True))
    sys.stdout.flush()


# ── instance state ───────────────────────────────────────────────────────────
def run_root():
    return Path(recorder.run_dir(os.getcwd()))


def inst_dir(name):
    return run_root() / name


def app_instance_name(name):
    """The UBILINK_INSTANCE value: the verifier's name plus a hash of this run dir, so
    two worktrees that both call their instance `a` never share an automation socket.
    Fits the app's [A-Za-z0-9_-]{1,32} rule (automation/socket.rs)."""
    h = hashlib.sha1(str(run_root().resolve()).encode()).hexdigest()[:6]
    return "vf-%s-%s" % (name, h)


def check_name(name):
    if not NAME_RE.match(name or ""):
        raise HarnessError("instance %r: use 1-16 chars of [a-z0-9-], starting with a letter"
                           % name)


def load_state(name):
    check_name(name)
    path = inst_dir(name) / "instance.json"
    try:
        doc = json.loads(path.read_text())
    except FileNotFoundError:
        raise HarnessError("no instance %r (no %s): launch it first" % (name, path))
    except ValueError as e:
        raise HarnessError("%s is not JSON: %s" % (path, e))
    if doc.get("status") != "running":
        raise HarnessError("instance %r is %s (launcher pid %s), not running"
                           % (name, doc.get("status"), doc.get("launcher_pid")))
    return doc


def pid_command(pid):
    """The executable path `ps` reports for pid, or None when it is not running."""
    r = subprocess.run(["ps", "-p", str(pid), "-o", "comm="], capture_output=True, text=True)
    out = r.stdout.strip()
    return out if r.returncode == 0 and out else None


def is_ours(doc):
    """The recorded pid is alive and still runs the recorded binary (not a reused pid)."""
    pid = doc.get("pid")
    return bool(pid) and pid_command(pid) == doc.get("binary")


# ── socket ───────────────────────────────────────────────────────────────────
class Conn:
    def __init__(self, doc):
        self.doc = doc
        s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        s.settimeout(probe._CALL_TIMEOUT_S)
        try:
            s.connect(doc["socket"])
        except OSError as e:
            s.close()
            raise HarnessError("instance %s: cannot connect to %s: %s"
                               % (doc["instance"], doc["socket"], e))
        self.sock, self.reader = s, s.makefile("rb")

    def request(self, verb, args=None):
        line = json.dumps({"verb": verb, "args": args or {}}) + "\n"
        try:
            self.sock.sendall(line.encode())
            reply = self.reader.readline()
        except OSError as e:
            raise HarnessError("instance %s: %s: %s" % (self.doc["instance"], verb, e))
        if not reply:
            raise HarnessError("instance %s closed the socket on %s" % (self.doc["instance"], verb))
        return json.loads(reply)

    def call(self, verb, args=None):
        resp = self.request(verb, args)
        if not resp.get("ok"):
            raise Failed("%s answered ok:false: %s" % (verb, resp.get("error")))
        return resp.get("result")

    def close(self):
        with contextlib.suppress(OSError):
            self.reader.close()
            self.sock.close()


def tokens(doc):
    return {"root": doc["root"], "data_dir": doc["data_dir"], "fixtures": doc["fixtures"],
            "instance": doc["app_instance"], "probes": str(probe.PROBES_DIR)}


# ── launch ───────────────────────────────────────────────────────────────────
def socket_answers(path):
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        s.settimeout(2)
        s.connect(str(path))
        return True
    except OSError:
        return False  # absent, or a stale file the app clears when it binds
    finally:
        s.close()


def cmd_launch(a):
    check_name(a.instance)
    d = inst_dir(a.instance)
    d.mkdir(parents=True, exist_ok=True)
    claim = d / "instance.json"
    try:
        fd = os.open(claim, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o644)
    except FileExistsError:
        try:
            old = json.loads(claim.read_text())
        except ValueError:
            old = {}
        if old.get("status") == "running" and is_ours(old):
            raise Blocked("instance %r is already running (pid %s): stop it or pick another name"
                          % (a.instance, old.get("pid")))
        if old.get("status") == "launching" and old.get("launcher_pid") and \
                pid_command(old["launcher_pid"]):
            raise Blocked("instance %r is being launched by pid %s"
                          % (a.instance, old["launcher_pid"]))
        err("replacing a stale claim for %r (%s)" % (a.instance, old.get("status")))
        claim.unlink()
        fd = os.open(claim, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o644)
    with os.fdopen(fd, "w") as f:
        json.dump({"status": "launching", "instance": a.instance,
                   "launcher_pid": os.getpid()}, f)
    inst = None
    try:
        build = probe.build_binary(d, out=err)
        candidate = probe.Instance(a.instance, build)
        # Deterministic name (probe's default embeds the launcher's pid, which is gone
        # after this command returns): the same isolation, an addressable socket.
        candidate.name = app_instance_name(a.instance)
        if socket_answers(candidate.socket_path):
            # Checked before `inst` is set, so the cleanup below never touches the
            # other owner's socket.
            raise Blocked("%s already answers: another owner runs %s"
                          % (candidate.socket_path, candidate.name))
        inst = candidate
        inst.start(timeout=a.timeout)
        st = inst.call("state")
        doc = {
            "status": "running", "instance": a.instance, "app_instance": inst.name,
            "pid": inst.proc.pid, "binary": str(build.path), "binary_sha256": build.sha256,
            "commit": build.commit, "dirty": build.dirty, "root": str(inst.root),
            "data_dir": str(inst.data_dir), "fixtures": str(inst.fixtures_dir),
            "socket": str(inst.socket_path), "si_socket": str(inst.si_socket_path),
            "log": str(inst.log_path), "started_at": recorder._now(),
            "ready": {"signal": "automation socket answered state", "data_dir": st.get("data_dir")},
            "network_port": None,
        }
        tmp = claim.with_suffix(".tmp")
        tmp.write_text(json.dumps(doc, indent=2, sort_keys=True) + "\n")
        os.replace(tmp, claim)
        # Detach: the instance outlives this command (it runs in its own session).
        inst._reader.close()
        inst._sock.close()
        emit({"INSTANCE": doc})
        return 0
    except BaseException:
        if inst is not None:
            inst.stop()
        with contextlib.suppress(OSError):
            claim.unlink()
        raise


# ── doctor (read-only) ───────────────────────────────────────────────────────
def cmd_doctor(a):
    doc = load_state(a.instance)
    checks, detail = {}, {}
    alive = is_ours(doc)
    checks["process"] = "pass" if alive else "fail"
    detail["process"] = "pid %s runs %s" % (doc["pid"], pid_command(doc["pid"]))
    head, dirty = probe.source_revision(REPO)
    try:
        sha_now = probe._sha256_file(Path(doc["binary"]))
    except OSError:
        sha_now = None
    build_ok = sha_now == doc["binary_sha256"] and doc["commit"] == head and not doc["dirty"] \
        and not dirty
    checks["build"] = "pass" if build_ok else "fail"
    detail["build"] = ("binary sha256 %s (recorded %s), built from %s%s, HEAD now %s%s"
                       % (sha_now, doc["binary_sha256"], doc["commit"],
                          "+dirty" if doc["dirty"] else "", head, "+dirty" if dirty else ""))
    state = None
    try:
        c = Conn(doc)
        try:
            state = c.call("state")
        finally:
            c.close()
    except (HarnessError, Failed) as e:
        detail["endpoint"] = str(e)
    owned = state is not None and state.get("data_dir") == doc["data_dir"]
    checks["endpoint"] = "pass" if owned else "fail"
    detail.setdefault("endpoint", "%s answers state with data_dir %s (recorded %s)" % (
        doc["socket"], (state or {}).get("data_dir"), doc["data_dir"]))
    shell = (state or {}).get("shell") or {}
    iso = owned and shell.get("hotkeys_skipped") is True and \
        doc["data_dir"].startswith(doc["root"])
    checks["isolation"] = "pass" if iso else "fail"
    detail["isolation"] = "hotkeys_skipped=%s, data_dir under the instance root=%s" % (
        shell.get("hotkeys_skipped"), doc["data_dir"].startswith(doc["root"]))
    try:
        screen = bool(probe.quartz("screen-access").get("granted"))
    except Exception as e:  # informational: a11y and state proofs need no grant
        screen = "unknown (%s)" % e
    ok = all(v == "pass" for v in checks.values())
    if a.as_args:
        print(" ".join(["--ok" if ok else "--fail"] +
                       ["--check %s=%s" % kv for kv in checks.items()]))
    else:
        emit({"doctor": {"instance": a.instance, "ok": ok, "checks": checks, "detail": detail,
                         "screen_recording": screen, "route": (state or {}).get("route")}})
    return 0 if ok else 1


# ── call ─────────────────────────────────────────────────────────────────────
def parse_step(text):
    verb, sep, raw = text.partition(":")
    if verb not in VERBS + CHECKS:
        raise HarnessError("unknown step %r: verbs are %s; checks are %s"
                           % (verb, ", ".join(VERBS), ", ".join(CHECKS)))
    if verb == "screenshot":
        label = raw or "panel"
        if not re.match(r"^[A-Za-z0-9_-]+\Z", label):
            raise HarnessError("screenshot label %r must be file-name safe" % label)
        return verb, label
    if not sep:
        if verb in ("wait_state", "see", "absent", "seed_graph"):
            raise HarnessError("%s needs JSON: %s:{...}" % (verb, verb))
        return verb, {}
    try:
        args = json.loads(raw)
    except ValueError as e:
        raise HarnessError("step %r: args are not JSON: %s" % (text, e))
    if not isinstance(args, dict) or (verb in ("wait_state", "see", "absent", "seed_graph")
                                      and not args):
        raise HarnessError("step %r: args must be a non-empty JSON object" % text)
    if verb in ("see", "absent"):
        try:
            probe.validate_step({"see": args, "absent": verb == "absent"}, verb)
        except probe.ScenarioError as e:
            raise HarnessError("step %r: %s" % (text, e))
    return verb, args


def poll(timeout_ms, attempt):
    deadline = time.monotonic() + timeout_ms / 1000.0
    while True:
        try:
            return attempt()
        except (Failed, probe.StepFailed) as e:
            if time.monotonic() >= deadline:
                raise Failed(str(e) + (" (expected %s, got %s)" % (
                    probe._short(e.expected), probe._short(e.actual))
                    if isinstance(e, probe.StepFailed) else ""))
        time.sleep(0.1)


def run_step(conn, doc, verb, args, out, n, timeout_ms):
    if verb in VERBS:
        resp = conn.request(verb, probe.expand(args, tokens(doc)))
        if not resp.get("ok"):
            raise Failed("%s answered ok:false: %s" % (verb, resp.get("error")))
        return resp
    if verb == "wait_state":
        want = probe.expand(args, tokens(doc))

        def attempt():
            st = conn.call("state")
            misses = probe.check_expect(st, want)
            if misses:
                raise Failed("state misses %s" % probe._short(misses))
            return st
        return {"ok": True, "result": poll(timeout_ms, attempt), "expect": want}
    if verb in ("see", "absent"):
        q = probe.expand(args, tokens(doc))

        def attempt():
            tree = conn.call("a11y")
            if verb == "absent" and not any(probe._norm_role(x["role"]) != "window"
                                            for x in probe.a11y_nodes(tree)):
                raise Failed("absent needs a rendered view, the tree has no non-Window node")
            probe.check_a11y_query(tree, q, absent=(verb == "absent"))
            return tree
        return {"ok": True, "result": poll(timeout_ms, attempt), "query": q}
    if verb == "seed_graph":
        # Relative to this instance's fixture copy, as probe.py's setup.graph resolves it.
        src = Path(probe.expand(args.get("fixture", ""), tokens(doc)))
        src = src if src.is_absolute() else Path(doc["fixtures"]) / src
        if not src.is_file():
            raise HarnessError("seed_graph: no fixture %s" % src)
        db = Path(doc["data_dir"]) / probe.GRAPH_DB_NAME
        nres, nlinks = probe.seed_graph(db, src, replace=bool(args.get("replace")))
        return {"ok": True, "result": {"db": str(db), "fixture": str(src),
                                       "resources": nres, "links": nlinks}}
    # screenshot
    if not probe.quartz("screen-access").get("granted"):
        raise Blocked("Screen Recording is not granted to the process running this harness "
                      "(System Settings > Privacy & Security > Screen Recording)")
    wid = conn.call("window_id")["window_id"]
    target = (Path(out) if out else Path(doc["root"]) / "shots") / ("%02d-screenshot-%s.png"
                                                                    % (n, args))
    target.parent.mkdir(parents=True, exist_ok=True)
    r = subprocess.run(["screencapture", "-x", "-o", "-l%d" % wid, str(target)],
                       capture_output=True, text=True)
    if r.returncode != 0 or not target.is_file() or target.stat().st_size == 0:
        raise Blocked("screencapture -l%d produced no image: %s" % (wid, r.stderr.strip()))
    return {"ok": True, "result": {"window_id": wid, "png": str(target),
                                   "bytes": target.stat().st_size}}


def next_index(out):
    if not out:
        return 1
    used = [int(m.group(1)) for p in Path(out).glob("*")
            for m in [re.match(r"^(\d+)-", p.name)] if m]
    return max(used, default=0) + 1


def cmd_call(a):
    doc = load_state(a.instance)
    steps = [parse_step(s) for s in a.steps]
    if a.out:
        Path(a.out).mkdir(parents=True, exist_ok=True)
    # The desktop is shared (key window, pasteboard, cursor): hold probe.py's desktop
    # lock for the whole step list, as `probe.py run` does for a scenario.
    with probe.desktop_lock(out=err):
        conn = Conn(doc)
        try:
            n = next_index(a.out)
            for verb, args in steps:
                line = {"step": n, "verb": verb, "args": args, "instance": a.instance}
                try:
                    resp = run_step(conn, doc, verb, args, a.out, n, a.timeout_ms)
                except Failed as e:
                    line.update(ok=False, error=str(e))
                    _save(a.out, n, verb, line)
                    emit(line)
                    return 1
                line.update(ok=True, result=resp.get("result"))
                for k in ("expect", "query"):
                    if k in resp:
                        line[k] = resp[k]
                _save(a.out, n, verb, line)
                emit(line if verb != "a11y" and verb not in ("see", "absent") or a.full
                     else dict(line, result="(a11y tree: %d nodes; saved)" % len(
                         probe.a11y_nodes(line["result"]))))
                n += 1
        finally:
            conn.close()
    return 0


def _save(out, n, verb, line):
    if not out:
        return
    path = Path(out) / ("%02d-%s.json" % (n, verb))
    path.write_text(json.dumps(line, indent=2, sort_keys=True) + "\n")
    with open(Path(out) / "transcript.jsonl", "a") as f:
        f.write(json.dumps({k: v for k, v in line.items() if k != "result"},
                           sort_keys=True) + "\n")


# ── stop ─────────────────────────────────────────────────────────────────────
def cmd_stop(a):
    check_name(a.instance)
    d = inst_dir(a.instance)
    claim = d / "instance.json"
    try:
        doc = json.loads(claim.read_text())
    except FileNotFoundError:
        raise HarnessError("no instance %r under %s" % (a.instance, run_root()))
    except ValueError:
        doc = {}
    killed = None
    pid = doc.get("pid")
    if pid and is_ours(doc):
        os.kill(pid, signal.SIGTERM)  # by pid, only after checking it runs our binary
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline and pid_command(pid):
            time.sleep(0.1)
        if pid_command(pid) == doc.get("binary"):
            os.kill(pid, signal.SIGKILL)
            killed = "SIGKILL"
        else:
            killed = "SIGTERM"
    # Instance.stop's cleanup: the socket, its 0700 dir, the temp root (data copy,
    # fixtures, app log). None of it is evidence: evidence was copied into .verify/
    # by verify_evidence.py record.
    for p in (doc.get("socket"), doc.get("si_socket")):
        if p:
            with contextlib.suppress(OSError):
                os.unlink(p)
    if doc.get("socket"):
        with contextlib.suppress(OSError):
            os.rmdir(os.path.dirname(doc["socket"]))
    root = doc.get("root")
    if root and os.path.basename(root).startswith("ubprobe-"):
        shutil.rmtree(root, ignore_errors=True)
    if d.resolve().parent == run_root().resolve() and d.name == a.instance:
        shutil.rmtree(d, ignore_errors=True)
    alive = bool(pid and pid_command(pid) == doc.get("binary"))
    emit({"STOPPED": {"instance": a.instance, "pid": pid, "signal": killed,
                      "still_running": alive, "removed": [str(d), root]}})
    return 2 if alive else 0


def cmd_list(a):
    root = run_root()
    for claim in sorted(root.glob("*/instance.json")):
        try:
            doc = json.loads(claim.read_text())
        except ValueError:
            doc = {"status": "unreadable"}
        emit({"instance": claim.parent.name, "status": doc.get("status"),
              "pid": doc.get("pid"), "alive": is_ours(doc), "socket": doc.get("socket")})
    return 0


def main(argv=None):
    p = argparse.ArgumentParser(prog="ubilink_instance.py", description=__doc__.split("\n")[0])
    sub = p.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("launch")
    s.add_argument("--instance", required=True)
    s.add_argument("--timeout", type=float, default=probe._LAUNCH_TIMEOUT_S,
                   help="seconds to wait for the automation socket (default 60)")
    s = sub.add_parser("doctor")
    s.add_argument("--instance", required=True)
    s.add_argument("--as-args", action="store_true",
                   help="print verify_evidence.py doctor arguments instead of JSON")
    s = sub.add_parser("call")
    s.add_argument("--instance", required=True)
    s.add_argument("--out", help="directory for one JSON file per step (+ screenshots)")
    s.add_argument("--timeout-ms", type=int, default=DEFAULT_TIMEOUT_MS,
                   help="how long wait_state/see/absent poll (default 5000)")
    s.add_argument("--full", action="store_true", help="print a11y trees on stdout too")
    s.add_argument("steps", nargs="+")
    s = sub.add_parser("stop")
    s.add_argument("--instance", required=True)
    sub.add_parser("list")
    a = p.parse_args(argv)
    fn = {"launch": cmd_launch, "doctor": cmd_doctor, "call": cmd_call, "stop": cmd_stop,
          "list": cmd_list}[a.cmd]
    try:
        return fn(a)
    except HarnessError as e:
        err(str(e))
        return e.code
    except probe.ProbeError as e:
        err(str(e))
        return 2
    except probe.EnvBlocked as e:
        err(str(e))
        return 3


if __name__ == "__main__":
    sys.exit(main())
