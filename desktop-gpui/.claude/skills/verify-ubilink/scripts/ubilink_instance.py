#!/usr/bin/env python3
"""ubilink_instance.py: long-lived, named ubilink-next instances for verify-ubilink.

`scripts/probe.py run` launches instances, runs one scenario and tears them down in a
single process. A verifier needs the same instances to outlive one command: launch,
doctor, drive, record evidence, clean up. This harness is that split of probe.py's own
engine. It imports the scripts/probe.py of the checkout that holds this skill and uses
its build (`build_binary`: the warm locked target and a per-run binary copy), its
isolation (`Instance`: own UBILINK_INSTANCE automation socket, own
UBILINK_INSTANCE_SOCKET, own UBILINK_DATA_DIR, own fixture copy,
UBILINK_SKIP_HOTKEYS=1), its desktop lock, and its step runners, matchers and
focus-theft guard. Steps run through probe.py's own runners (`STEP_RUNNERS`), so a step
here means exactly what it means in a probes/*.json scenario. It adds no semantics to
probe.py and changes none.

Usage (JSON on stdout; build and lock chatter on stderr):
    ubilink_instance.py name   --verifier V SUFFIX       # a verifier-unique instance name
    ubilink_instance.py launch --instance I --verifier V # INSTANCE: {...}   exit 0|2|3
    ubilink_instance.py doctor --instance I [--as-args]  # read-only checks  exit 0|1|2
    ubilink_instance.py call   --instance I --verifier V [--out DIR] STEP [STEP ...]
                                                          #                  exit 0|1|2|3
    ubilink_instance.py restart --instance I --verifier V
                          # RESTARTED: {...}: quit + relaunch, data dir and fixtures kept
                          #                                          exit 0|1|2|3
    ubilink_instance.py stop   --instance I --verifier V # STOPPED: {...}    exit 0|2|3
    ubilink_instance.py list                             # every instance under the run dir

STEP, each translated to a probe.py scenario step and validated by probe.py:
    verb | verb:JSON-args           {"call": verb, "args": ...}  (the 11 automation verbs)
    wait_state:{path: matcher}      {"call": "state", "expect": ..., "retry_ms": timeout}
    see:QUERY / absent:QUERY        {"see": QUERY, "timeout_ms": timeout[, "absent": true]}
    pasteboard:set_nonce            {"pasteboard": "set_nonce"}
    pasteboard:{"expect"?, "changed"?}   {"pasteboard": "read", ...}
    screenshot[:label]              the panel by window_id, saved into --out
    seed_graph:{"fixture": F, "replace"?}  setup only: probe.py's seed_graph
    step:{any probe.py step}        e.g. {"call": "resign_key", "expect": {"closed": true}},
                                    {"winframe": "gone"}, {"activate_other": true}
Strings may use {root}, {data_dir}, {fixtures}, {instance} and, after set_nonce, {nonce}.

One call is one proof. It holds probe.py's desktop lock throughout, first closes any
open panel (probe.py's reset_panel), and restores the pasteboard text at the end. When a
visibility step fails while another app took focus (probe.py's focus_thief), the whole
call is re-run once from a closed panel; a second theft exits 3 (ENV-BLOCKED), never 1.

Where state lives: <run dir>/<instance>/instance.json, where <run dir> is the
.verify-run/ next to the evidence directory (verify_evidence.py's run_dir for this
checkout: $VERIFY_EVIDENCE_DIR's sibling, else <checkout>/.verify-run). Two locks keep
owners apart: the instance claim (O_EXCL, names its verifier), and the session lease
<probe cache>/verify-lease.json (O_EXCL, names one run dir + verifier + its instances),
so only one verifier at a time drives instances on this desktop. This harness never
reads, writes or deletes the evidence directory.

Lease heartbeat and reclaim: every launch, call (after each step) and doctor of the
lease holder refreshes the lease's `last_activity` and marks its own pid active while it
runs. A lease with no holder command running and no activity for LEASE_IDLE_S (600 s)
may be reclaimed by a waiting `launch`: it stops the holder's instances by their
recorded pids (each only if it still runs our binary), removes their claims, writes
<holder run dir>/reclaims/<time>-<verifier>.json, says so on stderr, and takes the
lease. The holder's next command on a reclaimed instance exits 3 ("lease reclaimed
after idle") and drives nothing. Decisions: `lease_decision`, unit-tested in
scripts/test_ubilink_instance.py.

Every exit path of every command maps to these codes (main); a traceback is a harness
bug and exits 2.

Exit codes: 0 ok; 1 the product did not do what a step expects, or doctor found a failed
check; 2 harness error (bad input, no such instance, build failure, socket gone); 3
environment-blocked (instance or lease held by another owner, Screen Recording missing,
focus stolen twice, display asleep).
"""
import argparse
import contextlib
import ctypes
import hashlib
import importlib.util
import io
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
from types import SimpleNamespace

HERE = Path(__file__).resolve().parent
# <checkout>/desktop-gpui/.claude/skills/verify-ubilink/scripts -> <checkout>
REPO = HERE.parents[4]
PROBE_PY = REPO / "scripts" / "probe.py"
NAME_RE = re.compile(r"^[a-z][a-z0-9-]{0,15}\Z")
VERBS = ("open_view", "key", "state", "metrics", "trigger_hotkey", "resign_key",
         "set_appearance", "load_fixture", "window_id", "a11y", "opened")
# probe.py steps that need the scenario runner's own process or build: not here.
REFUSED_STEPS = ("restart", "process", "release_budget", "perf_doc")
DEFAULT_TIMEOUT_MS = 5000
LEASE_TIMEOUT_S = 900.0


def _load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    mod = importlib.util.module_from_spec(spec)
    sys.modules[name] = mod
    spec.loader.exec_module(mod)
    return mod


probe = _load("probe", PROBE_PY)
recorder = _load("verify_evidence", HERE / "verify_evidence.py")
LEASE = probe.CACHE_ROOT / "verify-lease.json"
LEASE_LOCK = probe.CACHE_ROOT / "verify-lease.lock"


class HarnessError(Exception):
    code = 2


class Blocked(HarnessError):
    code = 3


def err(msg):
    sys.stderr.write("ubilink_instance: %s\n" % msg)


def emit(obj):
    print(json.dumps(obj, sort_keys=True, default=str))
    sys.stdout.flush()


def quiet(msg):
    err(msg)


# ── names and claims ─────────────────────────────────────────────────────────
def run_root():
    return Path(recorder.run_dir(str(REPO))).resolve()


def inst_dir(name):
    return run_root() / name


def app_instance_name(name):
    """UBILINK_INSTANCE: the instance name plus a hash of the run dir, so two run dirs
    that both use a name never share an automation socket. Fits the app's
    [A-Za-z0-9_-]{1,32} rule (automation/socket.rs)."""
    h = hashlib.sha1(str(run_root()).encode()).hexdigest()[:6]
    return "vf-%s-%s" % (name, h)


def check_name(name):
    if not NAME_RE.match(name or ""):
        raise HarnessError("instance %r: use 1-16 chars of [a-z0-9-], starting with a letter"
                           % name)


def check_verifier(v):
    if not (v or "").strip():
        raise HarnessError("--verifier is required and cannot be blank")


def read_json(path):
    try:
        return json.loads(Path(path).read_text())
    except FileNotFoundError:
        return None
    except ValueError:
        return {}


def load_state(name, verifier=None):
    check_name(name)
    path = inst_dir(name) / "instance.json"
    doc = read_json(path)
    if doc is None:
        blocked_if_reclaimed(name, verifier)
        raise HarnessError("no instance %r (no %s): launch it first" % (name, path))
    if doc.get("status") != "running":
        raise HarnessError("instance %r is %s, not running" % (name, doc.get("status")))
    if verifier is not None and doc.get("verifier") != verifier:
        raise Blocked("instance %r belongs to verifier %r, not %r: use your own instance"
                      % (name, doc.get("verifier"), verifier))
    return doc


def pid_command(pid):
    """The executable path `ps` reports for pid, or None when it is not running."""
    r = subprocess.run(["ps", "-p", str(pid), "-o", "comm="], capture_output=True, text=True)
    out = r.stdout.strip()
    return out if r.returncode == 0 and out else None


def is_ours(doc):
    """The recorded pid is alive and still runs the recorded binary (not a reused pid)."""
    pid = (doc or {}).get("pid")
    return bool(pid) and pid_command(pid) == doc.get("binary")


def git_head(path):
    r = subprocess.run(["git", "-C", str(path), "rev-parse", "HEAD"], capture_output=True,
                       text=True)
    return r.stdout.strip() if r.returncode == 0 else None


def screen_locked():
    """True when the login session's screen is locked (CGSessionCopyCurrentDictionary's
    CGSSessionScreenIsLocked), None when it cannot be read. A locked screen keeps every
    panel from becoming active, so a11y reads fail for a reason that is not the
    product's: callers treat it as ENV-BLOCKED."""
    try:
        cg = ctypes.CDLL("/System/Library/Frameworks/CoreGraphics.framework/CoreGraphics")
        cf = ctypes.CDLL("/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation")
        cg.CGSessionCopyCurrentDictionary.restype = ctypes.c_void_p
        cf.CFStringCreateWithCString.restype = ctypes.c_void_p
        cf.CFStringCreateWithCString.argtypes = [ctypes.c_void_p, ctypes.c_char_p,
                                                 ctypes.c_uint32]
        cf.CFDictionaryGetValue.restype = ctypes.c_void_p
        cf.CFDictionaryGetValue.argtypes = [ctypes.c_void_p, ctypes.c_void_p]
        cf.CFGetTypeID.argtypes = [ctypes.c_void_p]
        cf.CFGetTypeID.restype = ctypes.c_ulong
        cf.CFBooleanGetTypeID.restype = ctypes.c_ulong
        cf.CFBooleanGetValue.argtypes = [ctypes.c_void_p]
        cf.CFBooleanGetValue.restype = ctypes.c_bool
        cf.CFRelease.argtypes = [ctypes.c_void_p]
        d = cg.CGSessionCopyCurrentDictionary()
        if not d:
            return None
        key = cf.CFStringCreateWithCString(None, b"CGSSessionScreenIsLocked", 0x08000100)
        v = cf.CFDictionaryGetValue(d, key)
        locked = bool(v) and cf.CFGetTypeID(v) == cf.CFBooleanGetTypeID() and \
            cf.CFBooleanGetValue(v)
        cf.CFRelease(key)
        cf.CFRelease(d)
        return locked
    except (OSError, AttributeError):
        return None


# ── session lease ────────────────────────────────────────────────────────────
# A lease whose holder has done nothing for this long, with no harness command of its
# running, may be reclaimed by a waiting verifier. 10 minutes because:
#  - every launch, call and doctor of the holder marks the lease active (its pid) while
#    it runs and refreshes `last_activity` (a call refreshes it after every step), so a
#    slow command (a cold build, a 15-minute desktop-lock wait, a proof with long
#    polls) never counts as idle however long it takes;
#  - what is left is the gap between two commands of a live verifier: reading output
#    and deciding the next step, normally well under 2 minutes, so 10 minutes is a
#    wide margin before treating the holder as gone;
#  - it is shorter than the 900 s a waiting `launch` waits (--lease-timeout), so a
#    re-dispatched verifier reclaims a crashed one's lease inside its own wait.
LEASE_IDLE_S = 600.0


def _epoch():
    return time.time()


def lease_owner():
    return LEASE, read_json(LEASE)


def lease_instances_alive(doc):
    root = Path(doc.get("run_root") or "/nonexistent")
    return [n for n in doc.get("instances") or []
            if is_ours(read_json(root / n / "instance.json"))]


def pid_alive(pid):
    return bool(pid) and pid_command(pid) is not None


def lease_decision(lease, me, now, alive_instances, pid_alive, idle_s=LEASE_IDLE_S):
    """What a launch by `me` ({run_root, verifier}) does about `lease`. Pure, so the
    rules are unit-tested (scripts/test_ubilink_instance.py):
      take      no lease
      join      the lease is ours (same run dir and verifier)
      stale     not ours; no harness command of the holder running and none of its
                instances alive: replace it, nothing to stop
      reclaim   not ours; no harness command running, instances alive, but idle for
                longer than idle_s: stop its instances, then take it
      wait      not ours and in use (a command running, or active within idle_s)
    `alive_instances` is the list of the lease's instances still running our binary;
    `pid_alive(pid)` says whether a holder pid is a running process."""
    if not lease:
        return "take"
    if lease.get("run_root") == me["run_root"] and lease.get("verifier") == me["verifier"]:
        return "join"
    if any(pid_alive(lease.get(k)) for k in ("launcher_pid", "active_pid")):
        return "wait"
    if not alive_instances:
        return "stale"
    last = lease.get("last_activity")
    if not isinstance(last, (int, float)):
        last = 0  # a lease written before heartbeats existed: idle since forever
    return "reclaim" if now - last > idle_s else "wait"


def _write_lease(doc, exclusive):
    LEASE.parent.mkdir(parents=True, exist_ok=True)
    if exclusive:
        fd = os.open(LEASE, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o644)
        with os.fdopen(fd, "w") as f:
            json.dump(doc, f, indent=2, sort_keys=True)
        return
    tmp = LEASE.with_suffix(".tmp")
    tmp.write_text(json.dumps(doc, indent=2, sort_keys=True) + "\n")
    os.replace(tmp, LEASE)


def reclaim_dir(root):
    return Path(root) / "reclaims"


def reclaim_lease(doc, by):
    """Stop the idle holder's instances (each only if its pid still runs our binary),
    remove their claims, and leave a record under the holder's run dir that its
    returning commands report. Caller holds LEASE_LOCK."""
    root = Path(doc["run_root"])
    stopped = []
    for n in doc.get("instances") or []:
        d = root / n
        claim = read_json(d / "instance.json")
        if claim:
            stopped.append({"instance": n, "pid": claim.get("pid"),
                            "signal": stop_instance(claim)})
        if d.parent == root and d.name == n:
            shutil.rmtree(d, ignore_errors=True)
    idle = _epoch() - (doc.get("last_activity") or 0)
    rec = {"kind": "lease-reclaimed", "at": recorder._now(), "by_verifier": by,
           "by_run_root": str(run_root()), "from_verifier": doc.get("verifier"),
           "run_root": doc.get("run_root"), "idle_s": round(idle), "idle_limit_s": LEASE_IDLE_S,
           "instances": doc.get("instances") or [], "stopped": stopped}
    reclaim_dir(root).mkdir(parents=True, exist_ok=True)
    path = reclaim_dir(root) / ("%s-%s.json" % (time.strftime("%Y%m%dT%H%M%S"),
                                                re.sub(r"[^A-Za-z0-9_.-]", "_",
                                                       str(doc.get("verifier")))))
    path.write_text(json.dumps(rec, indent=2, sort_keys=True) + "\n")
    err("RECLAIMED the session lease of verifier %r (idle %ds > %ds, no command of it "
        "running): stopped %s; record %s"
        % (doc.get("verifier"), idle, LEASE_IDLE_S,
           ", ".join("%s pid %s (%s)" % (x["instance"], x["pid"], x["signal"])
                     for x in stopped) or "nothing", path))
    with contextlib.suppress(OSError):
        LEASE.unlink()


def reclaimed_note(instance, verifier=None):
    """The reclaim record naming this instance (of this verifier), or None."""
    for path in sorted(reclaim_dir(run_root()).glob("*.json"), reverse=True):
        rec = read_json(path) or {}
        if instance in (rec.get("instances") or []) and \
                (verifier is None or rec.get("from_verifier") == verifier):
            return rec, path
    return None


def blocked_if_reclaimed(instance, verifier=None):
    hit = reclaimed_note(instance, verifier)
    if hit:
        rec, path = hit
        raise Blocked("lease reclaimed after idle: verifier %r took the session lease at %s "
                      "after %ss without activity and stopped instance %r (record %s). "
                      "Nothing was driven; launch again to continue"
                      % (rec.get("by_verifier"), rec.get("at"), rec.get("idle_s"), instance,
                         path))


def lease_acquire(verifier, instance, timeout):
    """Join our own lease, take a free or stale one, reclaim an idle one; wait while
    another verifier is using it."""
    me = {"run_root": str(run_root()), "verifier": verifier}
    deadline = time.monotonic() + timeout
    said = False
    while True:
        with probe.file_lock(LEASE_LOCK, "verify lease lock", out=quiet):
            doc = read_json(LEASE)
            alive = lease_instances_alive(doc) if doc else []
            what = lease_decision(doc, me, _epoch(), alive, pid_alive)
            if what == "join":
                doc["instances"] = sorted(set(doc.get("instances") or []) | {instance})
                doc.update(launcher_pid=os.getpid(), last_activity=_epoch())
                _write_lease(doc, exclusive=False)
                return
            if what == "stale":
                err("replacing a stale session lease held by %r (%s): none of its "
                    "instances is running" % (doc.get("verifier"), doc.get("run_root")))
                with contextlib.suppress(OSError):
                    LEASE.unlink()
            if what == "reclaim":
                reclaim_lease(doc, verifier)
            if what in ("take", "stale", "reclaim"):
                _write_lease(dict(me, instances=[instance], launcher_pid=os.getpid(),
                                  active_pid=None, last_activity=_epoch(),
                                  since=recorder._now()), exclusive=True)
                return
        if not said:
            idle = _epoch() - (doc.get("last_activity") or 0)
            err("waiting for the session lease %s, held by verifier %r for %s (instances "
                "%s, idle %ds; reclaimable after %ds idle with no command running)"
                % (LEASE, doc.get("verifier"), doc.get("run_root"), doc.get("instances"),
                   idle, LEASE_IDLE_S))
            said = True
        if time.monotonic() > deadline:
            raise Blocked("session lease %s still held by verifier %r after %.0fs"
                          % (LEASE, doc.get("verifier"), timeout))
        time.sleep(1.0)


def lease_update(verifier, drop=None, launcher_done=False):
    with probe.file_lock(LEASE_LOCK, "verify lease lock", out=quiet):
        doc = read_json(LEASE)
        if not doc or doc.get("run_root") != str(run_root()) or doc.get("verifier") != verifier:
            return "not held by this verifier"
        if drop:
            doc["instances"] = [n for n in doc.get("instances") or [] if n != drop]
        if launcher_done:
            doc["launcher_pid"] = None
        if not doc.get("instances") and not doc.get("launcher_pid"):
            LEASE.unlink()
            return "released"
        doc["last_activity"] = _epoch()
        _write_lease(doc, exclusive=False)
        return "kept for %s" % ", ".join(doc["instances"])


def lease_touch(doc, active):
    """Heartbeat from the holder's call/doctor: check the lease is still this
    instance's verifier's, refresh last_activity and mark (active=True) or clear
    (False) this process as running. Raises Blocked when the lease is gone or someone
    else's; with a reclaim record, says so. Atomic with reclaim (both under
    LEASE_LOCK), so a reclaim never stops an instance mid-command."""
    with probe.file_lock(LEASE_LOCK, "verify lease lock", out=quiet):
        lease = read_json(LEASE)
        mine = lease and lease.get("run_root") == str(run_root()) \
            and lease.get("verifier") == doc.get("verifier") \
            and doc["instance"] in (lease.get("instances") or [])
        if not mine:
            if not active:
                return
            blocked_if_reclaimed(doc["instance"], doc.get("verifier"))
            raise Blocked("the session lease %s is not held for instance %r of verifier %r "
                          "(holder: %s): relaunch" % (LEASE, doc["instance"],
                                                      doc.get("verifier"),
                                                      lease and lease.get("verifier")))
        lease["last_activity"] = _epoch()
        lease["active_pid"] = os.getpid() if active else None
        _write_lease(lease, exclusive=False)


def lease_check(doc):
    lease_touch(doc, active=True)


# ── socket ───────────────────────────────────────────────────────────────────
class Remote:
    """A launched instance as probe.py's runners see an `Instance`: the same
    attributes and `request`/`call`, over the instance's socket. Every exchange is
    logged so the call can save it as evidence."""

    release = False

    def __init__(self, doc):
        self.doc = doc
        self.name = doc["app_instance"]
        self.label = doc["instance"]
        self.root = Path(doc["root"])
        self.data_dir = Path(doc["data_dir"])
        self.fixtures_dir = Path(doc["fixtures"])
        self.log_path = Path(doc["log"])
        self.proc = SimpleNamespace(pid=doc["pid"], poll=lambda: None if is_ours(doc) else 1)
        self._fixtures = {}
        self.log = []
        s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        s.settimeout(probe._CALL_TIMEOUT_S)
        try:
            s.connect(doc["socket"])
        except OSError as e:
            s.close()
            raise HarnessError("instance %s: cannot connect to %s: %s"
                               % (doc["instance"], doc["socket"], e))
        self._sock, self._reader = s, s.makefile("rb")

    def request(self, verb, args=None):
        line = json.dumps({"verb": verb, "args": args or {}}) + "\n"
        try:
            self._sock.sendall(line.encode())
            reply = self._reader.readline()
        except OSError as e:
            raise probe.ProbeError("instance %s: %s: %s" % (self.label, verb, e))
        if not reply:
            raise probe.ProbeError("instance %s closed the socket on %s" % (self.label, verb))
        resp = json.loads(reply)
        self.log.append({"verb": verb, "args": args or {}, "response": resp})
        if verb == "load_fixture" and resp.get("ok"):
            # Fixtures live in the app's memory; `restart` replays them, as probe.py's
            # Instance.restart does, so the claim remembers each one (kind -> path).
            record_fixture(self.doc, (args or {}).get("kind"), (args or {}).get("path"))
        return resp

    def call(self, verb, **args):
        resp = self.request(verb, args)
        if not resp.get("ok"):
            raise probe.CallError(resp.get("error"))
        return resp.get("result")

    def restart(self, *a, **k):
        raise probe.ScenarioError("restart is not available on a launched instance")

    def close(self):
        with contextlib.suppress(OSError):
            self._reader.close()
            self._sock.close()


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


# ── name / launch ────────────────────────────────────────────────────────────
def cmd_name(a):
    check_verifier(a.verifier)
    if not re.match(r"^[a-z0-9]{1,4}\Z", a.suffix):
        raise HarnessError("suffix %r: 1-4 chars of [a-z0-9]" % a.suffix)
    print("v%s-%s" % (hashlib.sha1(a.verifier.encode()).hexdigest()[:8], a.suffix))
    return 0


def cmd_launch(a):
    check_name(a.instance)
    check_verifier(a.verifier)
    d = inst_dir(a.instance)
    d.mkdir(parents=True, exist_ok=True)
    claim = d / "instance.json"
    try:
        fd = os.open(claim, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o644)
    except FileExistsError:
        old = read_json(claim) or {}
        if old.get("status") == "running" and is_ours(old):
            raise Blocked("instance %r is already running (pid %s, verifier %r)"
                          % (a.instance, old.get("pid"), old.get("verifier")))
        if old.get("status") == "launching" and old.get("launcher_pid") and \
                pid_command(old["launcher_pid"]):
            raise Blocked("instance %r is being launched by pid %s"
                          % (a.instance, old["launcher_pid"]))
        err("replacing a stale claim for %r (%s)" % (a.instance, old.get("status")))
        claim.unlink()
        fd = os.open(claim, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o644)
    with os.fdopen(fd, "w") as f:
        json.dump({"status": "launching", "instance": a.instance, "verifier": a.verifier,
                   "launcher_pid": os.getpid()}, f)
    inst = None
    leased = False
    try:
        lease_acquire(a.verifier, a.instance, a.lease_timeout)
        leased = True
        build = probe.build_binary(d, out=err)  # outside the desktop lock, as `run` does
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
        awake = probe.keep_awake()
        try:
            with probe.desktop_lock(out=err):  # a new instance's first frame is GUI work
                inst.start(timeout=a.timeout)
                st = inst.call("state")
        finally:
            if awake is not None:
                awake.terminate()
        doc = {
            "status": "running", "instance": a.instance, "verifier": a.verifier,
            "app_instance": inst.name, "pid": inst.proc.pid, "binary": str(build.path),
            "binary_sha256": build.sha256, "commit": build.commit, "dirty": build.dirty,
            "checkout": str(REPO), "root": str(inst.root), "data_dir": str(inst.data_dir),
            "fixtures": str(inst.fixtures_dir), "socket": str(inst.socket_path),
            "si_socket": str(inst.si_socket_path), "log": str(inst.log_path),
            "started_at": recorder._now(), "network_port": None,
            "ready": {"signal": "automation socket answered state",
                      "data_dir": st.get("data_dir")},
        }
        tmp = claim.with_suffix(".tmp")
        tmp.write_text(json.dumps(doc, indent=2, sort_keys=True) + "\n")
        os.replace(tmp, claim)
        # Detach: the instance outlives this command (it runs in its own session).
        inst._reader.close()
        inst._sock.close()
        lease_update(a.verifier, launcher_done=True)
        emit({"INSTANCE": doc})
        return 0
    except BaseException:
        if inst is not None:
            inst.stop()
        with contextlib.suppress(OSError):
            claim.unlink()
        with contextlib.suppress(OSError):
            d.rmdir()  # only when empty: a failed launch leaves no claim directory
        if leased:
            lease_update(a.verifier, drop=a.instance, launcher_done=True)
        raise


# ── doctor (read-only) ───────────────────────────────────────────────────────
def cmd_doctor(a):
    doc = load_state(a.instance)
    lease_touch(doc, active=True)  # heartbeat; exit 3 if the lease was reclaimed
    try:
        return _doctor(a, doc)
    finally:
        lease_touch(doc, active=False)


def _doctor(a, doc):
    checks, detail = {}, {}
    checks["process"] = "pass" if is_ours(doc) else "fail"
    detail["process"] = "pid %s runs %s" % (doc["pid"], pid_command(doc["pid"]))
    head, dirty = probe.source_revision(REPO)
    cwd_head = git_head(os.getcwd())
    try:
        sha_now = probe._sha256_file(Path(doc["binary"]))
    except OSError:
        sha_now = None
    build_ok = (sha_now == doc["binary_sha256"] and doc["commit"] == head == cwd_head
                and not doc["dirty"] and not dirty)
    checks["build"] = "pass" if build_ok else "fail"
    detail["build"] = (
        "binary sha256 %s (built %s) from %s%s; this skill's checkout %s is at %s%s; "
        "cwd %s is at %s%s" % (
            sha_now, doc["binary_sha256"], doc["commit"], "+dirty" if doc["dirty"] else "",
            REPO, head, "+dirty" if dirty else "", os.getcwd(), cwd_head,
            "" if cwd_head == head else " (MISMATCH: cd into the skill's checkout; the "
            "recorder binds evidence to --worktree's HEAD)"))
    state = None
    try:
        c = Remote(doc)
        try:
            state = c.call("state")
        finally:
            c.close()
    except (HarnessError, probe.ProbeError, probe.CallError) as e:
        detail["endpoint"] = str(e)
    owned = state is not None and state.get("data_dir") == doc["data_dir"]
    checks["endpoint"] = "pass" if owned else "fail"
    detail.setdefault("endpoint", "%s answers state with data_dir %s (recorded %s)" % (
        doc["socket"], (state or {}).get("data_dir"), doc["data_dir"]))
    shell = (state or {}).get("shell") or {}
    inside = doc["data_dir"].startswith(doc["root"])
    checks["isolation"] = "pass" if owned and shell.get("hotkeys_skipped") is True and inside \
        else "fail"
    detail["isolation"] = "hotkeys_skipped=%s, data_dir under the instance root=%s" % (
        shell.get("hotkeys_skipped"), inside)
    locked = screen_locked()
    try:
        displays = len(probe.active_displays())
    except (probe.EnvBlocked, probe.ProbeError):
        displays = 0
    # A sleeping display is fine (every call wakes it, as `probe.py run` does); a
    # locked screen is not: no panel can become active until someone unlocks it.
    checks["desktop"] = "pass" if locked is False else "fail"
    detail["desktop"] = "screen locked=%s; %d active display(s) now (call wakes a sleeping " \
        "one)" % (locked, displays)
    try:
        screen = bool(probe.quartz("screen-access").get("granted"))
    except Exception as e:  # informational: a11y and state proofs need no grant
        screen = "unknown (%s)" % e
    ok = all(v == "pass" for v in checks.values())
    if a.as_args:
        print(" ".join(["--worktree", str(REPO), "--ok" if ok else "--fail"] +
                       ["--check %s=%s" % kv for kv in checks.items()]))
    else:
        emit({"doctor": {"instance": a.instance, "verifier": doc.get("verifier"), "ok": ok,
                         "checks": checks, "detail": detail, "worktree": str(REPO),
                         "screen_recording": screen, "route": (state or {}).get("route")}})
    return 0 if ok else 1


# ── call ─────────────────────────────────────────────────────────────────────
def parse_step(text, timeout_ms):
    """CLI step -> (kind, probe.py step dict)."""
    verb, sep, raw = text.partition(":")
    if verb == "screenshot":
        label = raw or "panel"
        step = {"screenshot": label}
    elif verb == "pasteboard" and raw == "set_nonce":
        step = {"pasteboard": "set_nonce"}
    else:
        args = {}
        if sep:
            try:
                args = json.loads(raw)
            except ValueError as e:
                raise HarnessError("step %r: not JSON after the colon: %s" % (text, e))
            if not isinstance(args, dict):
                raise HarnessError("step %r: the JSON must be an object" % text)
        if verb in VERBS:
            step = {"call": verb, "args": args} if args else {"call": verb}
        elif verb == "wait_state":
            step = {"call": "state", "expect": args, "retry_ms": timeout_ms}
        elif verb in ("see", "absent"):
            step = {"see": args, "timeout_ms": timeout_ms}
            if verb == "absent":
                step["absent"] = True
        elif verb == "pasteboard":
            step = dict(args, pasteboard="read")
        elif verb == "seed_graph":
            step = {"seed_graph": args.get("fixture")}
            if "replace" in args:
                step["replace"] = args["replace"]
        elif verb == "step":
            step = args
        else:
            raise HarnessError("unknown step %r: verbs are %s; harness steps are wait_state, "
                               "see, absent, pasteboard, screenshot, seed_graph, step"
                               % (verb, ", ".join(VERBS)))
    try:
        kind = probe.validate_step(step, "step %r" % text)
    except probe.ScenarioError as e:
        raise HarnessError(str(e))
    if kind in REFUSED_STEPS:
        raise HarnessError("step %r: %s needs probe.py run's own instance; use a scenario"
                           % (text, kind))
    return kind, step


def make_screenshot_runner(out):
    def run(ctx, step):
        png, _ = probe._capture(ctx)  # EnvBlocked without Screen Recording
        dest_dir = Path(out) if out else ctx.inst.root / "shots"
        dest_dir.mkdir(parents=True, exist_ok=True)
        dest = dest_dir / ("%02d-screenshot-%s.png" % (ctx.step_no, step["screenshot"]))
        shutil.copy2(png, dest)
        ctx.last_png = str(dest)
        return "recorded %s" % dest
    return run


def next_index(out):
    if not out:
        return 1
    used = [int(m.group(1)) for p in Path(out).glob("*")
            for m in [re.match(r"^(\d+)-", p.name)] if m]
    return max(used, default=0) + 1


def save(out, n, kind, line, exchanges):
    if not out:
        return
    full = dict(line, exchanges=exchanges)
    (Path(out) / ("%02d-%s.json" % (n, kind))).write_text(
        json.dumps(full, indent=2, sort_keys=True, default=str) + "\n")
    with open(Path(out) / "transcript.jsonl", "a") as f:
        f.write(json.dumps(line, sort_keys=True, default=str) + "\n")


def attempt_call(remote, steps, out, start_no, attempt):
    """One pass over the steps from a closed panel. Returns (outcome, last line);
    outcome: ok | fail | retry | blocked."""
    runners = dict(probe.STEP_RUNNERS, screenshot=make_screenshot_runner(out))
    ctx = probe.RunContext(remote, {"steps": [s for _, s in steps], "_name": "verify"})
    ctx.own_pids.add(remote.doc["pid"])
    n = start_no
    remote.log.clear()
    reset = {"step": n, "kind": "reset_panel", "attempt": attempt}
    try:
        probe.reset_panel(ctx)
    except probe.CallError as e:
        # probe.py's rule (run_steps): the app answered but refused to close its panel,
        # so the proof cannot start from a closed panel: a product FAIL (exit 1).
        # Socket/harness trouble raises ProbeError instead and exits 2 (main).
        reset.update(ok=False, outcome="product-fail",
                     error="could not reset the panel: %s" % e)
        save(out, n, "reset_panel", reset, list(remote.log))
        emit(reset)
        return "fail", n
    reset.update(ok=True, detail="no panel open before the proof (probe.py reset_panel)")
    save(out, n, "reset_panel", reset, list(remote.log))
    emit(reset)
    for kind, step in steps:
        lease_touch(remote.doc, active=True)  # heartbeat per step; Blocked if lost
        n += 1
        ctx.step_no = n
        remote.log.clear()
        line = {"step": n, "kind": kind, "def": step, "attempt": attempt,
                "instance": remote.label}
        before = probe._frontmost() if probe.is_visibility_step(kind, step) else None
        try:
            detail = runners[kind](ctx, step)
        except probe.EnvBlocked as e:
            line.update(ok=False, outcome="env-blocked", error=str(e))
            save(out, n, kind, line, list(remote.log))
            emit(line)
            return "blocked", n
        except (probe.StepFailed, probe.CallError) as e:
            thief = None
            if isinstance(e, probe.StepFailed) and probe.is_visibility_step(kind, step):
                thief = probe.focus_thief(before, probe._frontmost(), ctx.owned_pids())
            msg = str(e)
            if isinstance(e, probe.StepFailed):
                msg += " (expected %s, got %s)" % (probe._short(e.expected),
                                                   probe._short(e.actual))
            outcome = ("retry" if attempt == 1 else "blocked") if thief else "fail"
            line.update(ok=False, outcome={"retry": "focus-stolen-retry",
                                           "blocked": "env-blocked",
                                           "fail": "product-fail"}[outcome], error=msg)
            if thief:
                line["focus_taken_by"] = thief
            save(out, n, kind, line, list(remote.log))
            emit(line)
            return outcome, n
        line.update(ok=True, detail=detail)
        results = [x["response"].get("result") for x in remote.log]
        if kind == "call" and results and step["call"] != "a11y":
            line["result"] = results[-1]
        save(out, n, kind, line, list(remote.log))
        emit(line)
    return "ok", n


def cmd_call(a):
    check_verifier(a.verifier)
    doc = load_state(a.instance, a.verifier)
    steps = [parse_step(s, a.timeout_ms) for s in a.steps]
    if a.out:
        Path(a.out).mkdir(parents=True, exist_ok=True)
    # The desktop is shared (key window, pasteboard, cursor): hold probe.py's desktop
    # lock for the whole proof, as `probe.py run` does for a scenario.
    # As `probe.py run` does: hold the display awake (caffeinate -d -i -u, which also
    # wakes a sleeping display) for the whole proof, and refuse to drive a desktop
    # with no active display (ENV-BLOCKED, exit 3), since an asleep display makes the
    # app inactive and every a11y read fail for a reason that is not the product's.
    lease_check(doc)  # before waiting on the desktop lock: waiting counts as activity
    awake = probe.keep_awake()
    try:
        return _call_locked(a, doc, steps, awake)
    finally:
        if awake is not None:
            awake.terminate()
        lease_touch(doc, active=False)


def _call_locked(a, doc, steps, awake):
    with probe.desktop_lock(out=err):
        lease_check(doc)
        probe.wait_for_display()
        probe.displays_or_blocked(probe.active_displays())  # EnvBlocked: exit 3
        if screen_locked():
            raise probe.EnvBlocked("the screen is locked: no panel can become active, "
                                   "so nothing can be proven until it is unlocked")
        remote = Remote(doc)
        try:
            n = next_index(a.out)
            for attempt in (1, 2):
                outcome, last = attempt_call(remote, steps, a.out, n, attempt)
                if outcome == "retry":
                    err("focus was taken during a visibility step: re-running the whole "
                        "proof once from a closed panel")
                    n = last + 1
                    continue
                break
        except probe.ScenarioError as e:
            raise HarnessError(str(e))
        finally:
            probe.stop_activators()
            probe.restore_pasteboard()
            remote.close()
    return {"ok": 0, "fail": 1, "blocked": 3}[outcome]


# ── restart ──────────────────────────────────────────────────────────────────
def write_claim(doc):
    path = inst_dir(doc["instance"]) / "instance.json"
    tmp = path.with_suffix(".tmp")
    tmp.write_text(json.dumps(doc, indent=2, sort_keys=True) + "\n")
    os.replace(tmp, path)


def record_fixture(doc, kind, path):
    """Remember a loaded fixture in the claim (kind -> path; a later load of the same
    kind replaces it, like the app's own fixture store and probe.py's _fixtures)."""
    if not kind:
        return
    claim = read_json(inst_dir(doc["instance"]) / "instance.json") or doc
    loaded = dict(claim.get("fixtures_loaded") or {})
    loaded[kind] = path
    claim["fixtures_loaded"] = loaded
    doc["fixtures_loaded"] = loaded
    write_claim(claim)


def restart_problem(doc, binary_sha):
    """Why this claim cannot be restarted (exit 2), or None. Pure, unit-tested."""
    root = doc.get("root") or ""
    if not os.path.basename(root).startswith("ubprobe-"):
        return "instance %r has no probe temp root to keep (%r)" % (doc.get("instance"), root)
    if not os.path.isdir(doc.get("data_dir") or ""):
        return "instance %r: its data dir %s is gone, nothing would survive the restart" % (
            doc.get("instance"), doc.get("data_dir"))
    if binary_sha != doc.get("binary_sha256"):
        return ("instance %r: binary %s changed since launch (sha256 %s, launched %s): a "
                "restart must run the same build" % (doc.get("instance"), doc.get("binary"),
                                                     binary_sha, doc.get("binary_sha256")))
    return None


def abandon_relaunch(inst, doc):
    """Kill a relaunched process that did not finish starting (probe.py's
    Instance._terminate: SIGTERM, SIGKILL after 5 s, unlink the socket files; the temp
    root stays) and mark the claim so call/doctor refuse it and stop only cleans up."""
    with contextlib.suppress(Exception):
        inst._terminate()
    doc.update(pid=None, status="failed-restart", failed_at=recorder._now())
    write_claim(doc)


def cmd_restart(a):
    """Quit and relaunch the app with the same temp root, env and binary, as probe.py's
    Instance.restart: the data dir survives, every fixture loaded so far is replayed."""
    check_verifier(a.verifier)
    doc = load_state(a.instance, a.verifier)
    lease_touch(doc, active=True)  # heartbeat; exit 3 if the lease is not ours
    awake = probe.keep_awake()
    try:
        try:
            sha = probe._sha256_file(Path(doc["binary"]))
        except OSError:
            sha = None
        why = restart_problem(doc, sha)
        if why:
            raise HarnessError(why)
        with probe.desktop_lock(out=err):  # a quit and a first frame are GUI work
            old_pid, was_running = doc["pid"], is_ours(doc)
            sig = terminate_process(doc)
            if is_ours(doc):
                raise HarnessError("instance %r: pid %s survived SIGKILL" % (a.instance, old_pid))
            inst = probe.Instance(doc["instance"], doc["binary"])
            inst.name = doc["app_instance"]  # same UBILINK_INSTANCE: same socket path
            inst.root = Path(doc["root"])    # same data dir, fixture copy, si socket, log
            inst._fixtures = dict(doc.get("fixtures_loaded") or {})
            try:
                inst._launch(a.timeout)  # waits for the socket, replays the fixtures
                st = inst.call("state")
            except BaseException as e:
                # Review I5: a relaunch that did not complete (no socket within the
                # timeout, a refused fixture replay, a socket error on `state`, an
                # interrupt) must not leave a GUI process the claim does not name:
                # `stop` would then delete the temp root under a live, orphaned app.
                # Kill it rather than record it, because an instance that never
                # finished starting is not one a proof can use, and the verifier's
                # next step is a fresh launch either way.
                abandon_relaunch(inst, doc)
                if isinstance(e, probe.CallError):
                    emit({"RESTARTED": {"instance": a.instance, "ok": False,
                                        "old_pid": old_pid, "new_pid": None,
                                        "error": "the app refused a fixture replay: %s" % e}})
                    return 1
                raise
            doc.update(pid=inst.proc.pid, started_at=recorder._now(),
                       restarts=int(doc.get("restarts") or 0) + 1)
            write_claim(doc)
            inst._reader.close()
            inst._sock.close()
        ok = st.get("data_dir") == doc["data_dir"]
        emit({"RESTARTED": {"instance": a.instance, "ok": ok, "old_pid": old_pid,
                            "was_running": was_running, "signal": sig,
                            "new_pid": doc["pid"], "restarts": doc["restarts"],
                            "data_dir": st.get("data_dir"), "data_dir_kept": ok,
                            "fixtures_replayed": inst._fixtures}})
        # The app resolving another data dir after a restart is the product's doing.
        return 0 if ok else 1
    finally:
        if awake is not None:
            awake.terminate()
        lease_touch(doc, active=False)


# ── scenario: the frozen acceptance scenarios as evidence (ruling R-z) ──────
SCENARIO_LINE = re.compile(
    r"^(PASS|FAIL|ENV-BLOCKED|RETRY|ERROR)\s+(\S+?)(?: \[(\d+)/(\d+)\]| \[side-by-side\])?"
    r"[ :](.*)$")


def _safe(text):
    return re.sub(r"[^A-Za-z0-9_.-]+", "-", str(text)).strip("-") or "x"


def parse_transcript(text):
    """probe.py's report lines -> per-step verdicts. The last line for a step wins
    (a RETRY followed by PASS is a PASS, marked retried); continuation lines (the
    indented expected/actual) are attached to the step above them."""
    steps, order, last = {}, [], None
    for line in text.splitlines():
        m = SCENARIO_LINE.match(line)
        if not m:
            if last is not None and line.startswith("      "):
                steps[last]["more"].append(line.strip())
            continue
        verdict, prefix, idx, total, rest = m.groups()
        if idx is None and "[side-by-side]" in line:
            key = (prefix, "side-by-side:" + rest.split(" — ")[0].strip())
        elif idx is None:
            key = (prefix, "scenario")
        else:
            key = (prefix, int(idx))
        prev = steps.get(key)
        entry = {"instance": prefix, "index": key[1], "total": int(total) if total else None,
                 "verdict": verdict if verdict != "RETRY" else "RETRY",
                 "text": rest.strip(), "more": [],
                 "retried": bool(prev and prev["verdict"] == "RETRY") or verdict == "RETRY"}
        if key not in steps:
            order.append(key)
        steps[key] = entry
        last = key
    return [steps[k] for k in order]


def make_claimed_instance_class(doc, verifier, out_dir, extra_names):
    """A probe.Instance subclass for run_scenario: the first instance it builds is the
    verifier's own (same UBILINK_INSTANCE, binary and claim; a fresh temp root, because
    every acceptance scenario starts from a fresh instance, as `probe.py run` gives it);
    later ones are extra instances claimed by this verifier for the command's length.
    Release-build instances (release_budget) stay plain probe.py instances, which that
    step starts and stops itself. Every launch (first start and each `restart` step)
    records the live pid in the claim, so `stop` always knows what runs."""
    base = probe.Instance
    made = []

    class ClaimedInstance(base):
        def __init__(self, label, binary, release=False):
            super().__init__(label, binary, release)
            self.log, self.ready, self.primary = [], False, False
            if release:
                self.claim = None
                return
            if not made:
                self.primary = True
                self.claim = dict(doc)
                self.name = doc["app_instance"]
            else:
                name = "%s-%d" % (doc["instance"][:13], len(made) + 1)
                check_name(name)
                extra_names.append(name)
                self.name = app_instance_name(name)
                self.claim = {"status": "launching", "instance": name, "verifier": verifier,
                              "app_instance": self.name, "binary": doc["binary"],
                              "binary_sha256": doc["binary_sha256"], "commit": doc["commit"],
                              "dirty": doc["dirty"], "checkout": str(REPO),
                              "network_port": None, "owned_by_scenario_of": doc["instance"]}
                inst_dir(name).mkdir(parents=True, exist_ok=True)
                write_claim(self.claim)
                lease_add(verifier, name)
            made.append(self)

        def _launch(self, timeout):
            if self.claim is None:
                return super()._launch(timeout)
            if self.primary and not self.ready and not getattr(self, "_replaced", False):
                # The verifier's running process and its old root give way to the fresh
                # one (same socket name: the app refuses to bind a live socket).
                old = dict(self.claim)
                stop_instance(old)
                self._replaced = True
            try:
                super()._launch(timeout)
            finally:
                if self.proc is not None:  # record whatever runs, even a failed start
                    self.claim.update(
                        status="running", pid=self.proc.pid, root=str(self.root),
                        data_dir=str(self.data_dir), fixtures=str(self.fixtures_dir),
                        socket=str(self.socket_path), si_socket=str(self.si_socket_path),
                        log=str(self.log_path), started_at=recorder._now(),
                        fixtures_loaded=dict(self._fixtures))
                    write_claim(self.claim)
            self.ready = True

        def request(self, verb, args=None):
            resp = super().request(verb, args)
            self.log.append({"verb": verb, "args": args or {}, "response": resp})
            if verb == "load_fixture" and resp.get("ok") and self.claim is not None:
                self.claim["fixtures_loaded"] = dict(self._fixtures)
                write_claim(self.claim)
            return resp

        def stop(self):
            if self.primary and self.ready and self.proc is not None \
                    and self.proc.poll() is None:
                # The verifier's own instance outlives the scenario (its claim names the
                # live process), for further observation; `stop` ends it.
                for f in (self._reader, self._sock):
                    with contextlib.suppress(OSError):
                        f and f.close()
                self._sock = self._reader = None
                return
            super().stop()
            if self.claim is not None and not self.primary:
                d = inst_dir(self.claim["instance"])
                if d.parent == run_root():
                    shutil.rmtree(d, ignore_errors=True)
                lease_update(verifier, drop=self.claim["instance"])
            elif self.primary:
                self.claim.update(pid=None, status="scenario-ended")
                write_claim(self.claim)

    return ClaimedInstance, made


def lease_add(verifier, name):
    with probe.file_lock(LEASE_LOCK, "verify lease lock", out=quiet):
        lease = read_json(LEASE)
        if not lease or lease.get("verifier") != verifier \
                or lease.get("run_root") != str(run_root()):
            raise Blocked("the session lease is not this verifier's: cannot add %r" % name)
        lease["instances"] = sorted(set(lease.get("instances") or []) | {name})
        lease["last_activity"] = _epoch()
        _write_lease(lease, exclusive=False)


def instrument_runners(doc, out_dir, records):
    """Wrap probe.py's step runners (the engine still runs them, with its own retry and
    tally) to save each step's socket exchanges and captured images under out_dir and
    to refresh the lease heartbeat. Returns a restore function."""
    original = dict(probe.STEP_RUNNERS)

    def wrap(kind, fn):
        def run(ctx, step):
            lease_touch(doc, active=True)
            inst = ctx.inst
            idx = next((i for i, st in enumerate(ctx.scenario["steps"], 1) if st is step), 0)
            prefix = _safe(getattr(inst, "label", "x").replace("#", "-"))
            shots = Path(inst.root) / "shots" if getattr(inst, "root", None) else None
            before = set(shots.glob("*.png")) if shots and shots.is_dir() else set()
            if hasattr(inst, "log"):
                inst.log.clear()
            rec = {"instance": getattr(inst, "label", None), "index": idx, "kind": kind,
                   "def": step, "started_at": recorder._now()}
            try:
                detail = fn(ctx, step)
                rec.update(outcome="pass", detail=detail)
                return detail
            except BaseException as e:
                rec.update(outcome=type(e).__name__, error=str(e))
                for k in ("expected", "actual"):
                    if hasattr(e, k):
                        rec[k] = getattr(e, k)
                raise
            finally:
                rec["exchanges"] = list(getattr(inst, "log", []))
                rec["saved"] = dict(getattr(ctx, "saved", {}) or {})
                n = sum(1 for r in records if (r["instance"], r["index"]) ==
                        (rec["instance"], idx))
                base = "%s-%02d-%s%s" % (prefix, idx, kind, "-retry%d" % n if n else "")
                if kind == "screenshot" and isinstance(rec.get("detail"), str):
                    src = Path(rec["detail"].replace("recorded ", "", 1))
                    if src.is_file():
                        dest = Path(out_dir) / ("%s-%s.png" % (base, _safe(step["screenshot"])))
                        shutil.move(str(src), dest)
                        rec["image"] = dest.name
                if shots and shots.is_dir():
                    for k, png in enumerate(sorted(set(shots.glob("*.png")) - before), 1):
                        dest = Path(out_dir) / ("%s-capture%d.png" % (base, k))
                        shutil.copy2(png, dest)
                        rec.setdefault("captures", []).append(dest.name)
                rec["file"] = base + ".json"
                (Path(out_dir) / rec["file"]).write_text(
                    json.dumps(rec, indent=2, sort_keys=True, default=str) + "\n")
                records.append(rec)
        return run

    for kind, fn in original.items():
        probe.STEP_RUNNERS[kind] = wrap(kind, fn)

    def restore():
        probe.STEP_RUNNERS.clear()
        probe.STEP_RUNNERS.update(original)
    return restore


class Tee:
    def __init__(self, *streams):
        self.streams = streams

    def write(self, text):
        for st in self.streams:
            st.write(text)
        return len(text)

    def flush(self):
        for st in self.streams:
            st.flush()


PROBES_DIR = probe.PROBES_DIR  # the frozen scenarios; tests point this elsewhere


def run_one_scenario(a, doc, sc, out_root):
    name = sc["_name"]
    out_dir = Path(out_root) / name
    out_dir.mkdir(parents=True, exist_ok=True)
    for old in out_dir.iterdir():  # one result per scenario per run; evidence is in .verify
        if old.is_file():
            old.unlink()
    records, extra = [], []
    cls, made = make_claimed_instance_class(doc, a.verifier, out_dir, extra)
    restore = instrument_runners(doc, out_dir, records)
    real_instance, real_shots = probe.Instance, probe._SHOTS_DIR
    probe.Instance, probe._SHOTS_DIR = cls, out_dir
    tally = probe.Tally()
    buf = io.StringIO()
    head, dirty = probe.source_revision(REPO)
    try:
        with contextlib.redirect_stdout(Tee(buf, sys.stderr)):
            probe.run_scenario(sc, Path(doc["binary"]), a.instances, tally)
    finally:
        probe.Instance, probe._SHOTS_DIR = real_instance, real_shots
        restore()
    code = tally.exit_code()
    transcript = buf.getvalue()
    (out_dir / "transcript.txt").write_text(transcript)
    primary = made[0].claim if made else doc
    if made:
        doc.update(made[0].claim)  # the verifier's instance now names the fresh process
    result = {
        "schema": "verify-ubilink-scenario/v1", "scenario": name,
        "scenario_file": str(sc["_path"]), "requirement": sc.get("requirement"),
        "description": sc.get("description"), "sha": head, "dirty": dirty,
        "binary": doc["binary"], "binary_sha256": doc["binary_sha256"],
        "instance": doc["instance"], "verifier": a.verifier, "instances": a.instances,
        "extra_instances": extra, "exit": code,
        "verdict": {0: "PASS", 1: "FAIL", 2: "ERROR", 3: "ENV-BLOCKED"}[code],
        "counts": {"passed": tally.passed, "failed": tally.failed,
                   "env_blocked": tally.blocked, "errors": tally.errors},
        "steps": parse_transcript(transcript),
        "step_records": [r["file"] for r in records],
        "saved": [dict(getattr(c, "saved", {}) or {}) for c in sc.get("_contexts", [])],
        "primary_pid_after": primary.get("pid"), "finished_at": recorder._now(),
    }
    result["artifacts"] = sorted(p.name for p in out_dir.iterdir() if p.is_file()) + \
        ["result.json"]
    (out_dir / "result.json").write_text(json.dumps(result, indent=2, sort_keys=True,
                                                    default=str) + "\n")
    emit({"SCENARIO": {"name": name, "exit": code, "verdict": result["verdict"],
                       "counts": result["counts"], "result": str(out_dir / "result.json"),
                       "artifacts": len(result["artifacts"])}})
    return code


def cmd_scenario(a):
    check_verifier(a.verifier)
    if a.instances < 1:
        raise HarnessError("--instances must be >= 1")
    for n in a.name:
        if "/" in n or n.endswith(".json"):
            raise HarnessError("--name takes a scenario name from desktop-gpui/probes "
                               "(the frozen acceptance scenarios), not a path: %r" % n)
    try:
        scenarios = [probe.load_scenario(n, PROBES_DIR) for n in a.name]  # read-only
    except probe.ScenarioError as e:
        raise HarnessError(str(e))
    doc = load_state(a.instance, a.verifier)
    lease_touch(doc, active=True)
    head, dirty = probe.source_revision(REPO)
    try:
        sha = probe._sha256_file(Path(doc["binary"]))
    except OSError:
        sha = None
    if sha != doc["binary_sha256"] or doc["commit"] != head or doc["dirty"] or dirty:
        lease_touch(doc, active=False)
        raise HarnessError("instance %r runs a build that is not this checkout's clean HEAD "
                           "(built %s%s, HEAD %s%s, binary sha256 %s): commit, stop and "
                           "launch again" % (a.instance, doc["commit"],
                                             "+dirty" if doc["dirty"] else "", head,
                                             "+dirty" if dirty else "", sha))
    out_root = Path(a.out) if a.out else inst_dir(a.instance) / "scenarios"
    run_dir = inst_dir(a.instance) / "scenario-run"
    run_dir.mkdir(parents=True, exist_ok=True)
    old_term = signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))
    awake = probe.keep_awake()
    real_run_dir = probe._RUN_DIR
    probe._RUN_DIR = run_dir  # release_budget builds its release copy here
    codes = []
    try:
        # As probe.py's run_loaded: helpers and builds before the desktop lock.
        probe._helper(probe._QUARTZ_SWIFT)
        kinds = {probe.step_type(st, "") for sc in scenarios for st in sc["steps"]}
        if "ocr" in kinds:
            probe._helper(probe._OCR_SWIFT)
        if "release_budget" in kinds:
            probe.build_binary(run_dir, release=True, out=err)
        if "activate_other" in kinds:
            probe._helper(probe._ACTIVATOR_SWIFT)
        probe.wait_for_display()
        with probe.desktop_lock(out=err):
            lease_check(doc)
            probe.displays_or_blocked(probe.active_displays())
            if screen_locked():
                raise probe.EnvBlocked("the screen is locked: no panel can become active, "
                                       "so nothing can be proven until it is unlocked")
            for sc in scenarios:
                codes.append(run_one_scenario(a, doc, sc, out_root))
    finally:
        probe.stop_activators()
        probe.restore_pasteboard()
        probe._RUN_DIR = real_run_dir
        shutil.rmtree(run_dir, ignore_errors=True)
        if awake is not None:
            awake.terminate()
        signal.signal(signal.SIGTERM, old_term)
        lease_touch(doc, active=False)
    # probe.py's Tally order across scenarios: an error beats a failure beats a block.
    for code in (2, 1, 3):
        if code in codes:
            return code
    return 0


# ── stop / list ──────────────────────────────────────────────────────────────
def terminate_process(doc):
    """Instance._terminate semantics for a claim: SIGTERM by the recorded pid, only
    after checking that pid still runs this instance's binary (never a reused pid,
    never by name), SIGKILL after 5 s, then unlink the automation and single-instance
    socket files (the app does not unlink them; a relaunch must never mistake a stale
    file for the new process). Keeps the temp root: data dir, fixture copy, app log.
    Returns the signal used, or None when the process was already gone."""
    killed = None
    pid = doc.get("pid")
    if pid and is_ours(doc):
        os.kill(pid, signal.SIGTERM)
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline and pid_command(pid):
            time.sleep(0.1)
        if pid_command(pid) == doc.get("binary"):
            os.kill(pid, signal.SIGKILL)
            killed = "SIGKILL"
        else:
            killed = "SIGTERM"
    for p in (doc.get("socket"), doc.get("si_socket")):
        if p:
            with contextlib.suppress(OSError):
                os.unlink(p)
    return killed


def stop_instance(doc):
    """Instance.stop semantics for a claim: terminate_process, then remove the
    socket's 0700 dir and the temp root (data copy, fixtures, app log). None of it is
    evidence. Returns the signal used, or None when the process was already gone."""
    killed = terminate_process(doc)
    if doc.get("socket"):
        with contextlib.suppress(OSError):
            os.rmdir(os.path.dirname(doc["socket"]))
    root = doc.get("root")
    if root and os.path.basename(root).startswith("ubprobe-"):
        shutil.rmtree(root, ignore_errors=True)
    return killed


def cmd_stop(a):
    check_name(a.instance)
    check_verifier(a.verifier)
    d = inst_dir(a.instance)
    claim = d / "instance.json"
    doc = read_json(claim)
    if doc is None:
        blocked_if_reclaimed(a.instance, a.verifier)
        raise HarnessError("no instance %r under %s" % (a.instance, run_root()))
    if doc.get("verifier") not in (None, a.verifier):
        raise Blocked("instance %r was launched by verifier %r, not %r: stop only your own"
                      % (a.instance, doc.get("verifier"), a.verifier))
    pid = doc.get("pid")
    root = doc.get("root")
    killed = stop_instance(doc)
    if d.resolve().parent == run_root() and d.name == a.instance:
        shutil.rmtree(d, ignore_errors=True)
    lease = lease_update(a.verifier, drop=a.instance)
    alive = bool(pid and pid_command(pid) == doc.get("binary"))
    emit({"STOPPED": {"instance": a.instance, "pid": pid, "signal": killed,
                      "still_running": alive, "lease": lease, "removed": [str(d), root]}})
    return 2 if alive else 0


def cmd_list(a):
    for claim in sorted(run_root().glob("*/instance.json")):
        doc = read_json(claim) or {"status": "unreadable"}
        emit({"instance": claim.parent.name, "verifier": doc.get("verifier"),
              "status": doc.get("status"), "pid": doc.get("pid"), "alive": is_ours(doc),
              "socket": doc.get("socket")})
    lease = read_json(LEASE)
    if lease:
        alive = lease_instances_alive(lease)
        emit({"lease": lease, "instances_alive": alive,
              "idle_s": round(_epoch() - (lease.get("last_activity") or 0)),
              "reclaimable_after_idle_s": LEASE_IDLE_S,
              "decision_for_another_verifier": lease_decision(
                  lease, {"run_root": "", "verifier": ""}, _epoch(), alive, pid_alive)})
    return 0


def main(argv=None):
    p = argparse.ArgumentParser(prog="ubilink_instance.py", description=__doc__.split("\n")[0])
    sub = p.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("name")
    s.add_argument("--verifier", required=True)
    s.add_argument("suffix")
    s = sub.add_parser("launch")
    s.add_argument("--instance", required=True)
    s.add_argument("--verifier", required=True)
    s.add_argument("--timeout", type=float, default=probe._LAUNCH_TIMEOUT_S,
                   help="seconds to wait for the automation socket (default 60)")
    s.add_argument("--lease-timeout", type=float, default=LEASE_TIMEOUT_S,
                   help="seconds to wait for another verifier's session lease (default 900)")
    s = sub.add_parser("doctor")
    s.add_argument("--instance", required=True)
    s.add_argument("--as-args", action="store_true",
                   help="print verify_evidence.py doctor arguments instead of JSON")
    s = sub.add_parser("call")
    s.add_argument("--instance", required=True)
    s.add_argument("--verifier", required=True)
    s.add_argument("--out", help="directory for one JSON file per step (+ screenshots)")
    s.add_argument("--timeout-ms", type=int, default=DEFAULT_TIMEOUT_MS,
                   help="how long wait_state/see/absent poll (default 5000)")
    s.add_argument("steps", nargs="+")
    s = sub.add_parser("scenario")
    s.add_argument("--instance", required=True)
    s.add_argument("--verifier", required=True)
    s.add_argument("--name", action="append", required=True,
                   help="a desktop-gpui/probes scenario name; repeat for several")
    s.add_argument("--instances", type=int, default=1,
                   help="run each scenario on N instances (yours + N-1 claimed for the "
                        "command) and add probe.py's side-by-side checks")
    s.add_argument("--out", help="artifacts go to OUT/<name>/ (default "
                                 ".verify-run/<instance>/scenarios/<name>/)")
    s = sub.add_parser("restart")
    s.add_argument("--instance", required=True)
    s.add_argument("--verifier", required=True)
    s.add_argument("--timeout", type=float, default=probe._LAUNCH_TIMEOUT_S,
                   help="seconds to wait for the relaunched socket (default 60)")
    s = sub.add_parser("stop")
    s.add_argument("--instance", required=True)
    s.add_argument("--verifier", required=True)
    sub.add_parser("list")
    a = p.parse_args(argv)
    fn = {"name": cmd_name, "launch": cmd_launch, "doctor": cmd_doctor, "call": cmd_call,
          "scenario": cmd_scenario, "restart": cmd_restart, "stop": cmd_stop, "list": cmd_list}[a.cmd]
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
    except probe.ScenarioError as e:
        err("bad step: %s" % e)
        return 2
    except probe.CallError as e:
        # An app refusal that escaped a step's own handling: the product answered no.
        err("the app refused: %s" % e)
        return 1
    except KeyboardInterrupt:
        err("interrupted")
        return 2
    except Exception:  # a harness bug is never a product verdict
        import traceback
        traceback.print_exc()
        err("harness error (see traceback): exit 2")
        return 2


if __name__ == "__main__":
    sys.exit(main())
