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
def lease_owner():
    return LEASE, read_json(LEASE)


def lease_stale(doc):
    """No launcher alive and none of the lease's instances alive."""
    if not doc:
        return True
    lp = doc.get("launcher_pid")
    if lp and pid_command(lp):
        return False
    root = Path(doc.get("run_root") or "/nonexistent")
    return not any(is_ours(read_json(root / n / "instance.json"))
                   for n in doc.get("instances") or [])


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


def lease_acquire(verifier, instance, timeout):
    """Join our own lease or take a free/stale one; wait while another verifier holds it."""
    me = {"run_root": str(run_root()), "verifier": verifier}
    deadline = time.monotonic() + timeout
    said = False
    while True:
        with probe.file_lock(LEASE_LOCK, "verify lease lock", out=quiet):
            doc = read_json(LEASE)
            if doc is not None and doc.get("run_root") == me["run_root"] \
                    and doc.get("verifier") == verifier:
                doc["instances"] = sorted(set(doc.get("instances") or []) | {instance})
                doc["launcher_pid"] = os.getpid()
                _write_lease(doc, exclusive=False)
                return
            if doc is not None and lease_stale(doc):
                err("replacing a stale session lease held by %r (%s)"
                    % (doc.get("verifier"), doc.get("run_root")))
                with contextlib.suppress(OSError):
                    LEASE.unlink()
                doc = None
            if doc is None:
                _write_lease(dict(me, instances=[instance], launcher_pid=os.getpid(),
                                  since=recorder._now()), exclusive=True)
                return
        if not said:
            err("waiting for the session lease %s, held by verifier %r for %s (instances %s)"
                % (LEASE, doc.get("verifier"), doc.get("run_root"), doc.get("instances")))
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
        _write_lease(doc, exclusive=False)
        return "kept for %s" % ", ".join(doc["instances"])


def lease_check(doc):
    lease = read_json(LEASE)
    if not lease or lease.get("run_root") != str(run_root()) \
            or lease.get("verifier") != doc.get("verifier") \
            or doc["instance"] not in (lease.get("instances") or []):
        raise Blocked("the session lease %s is not held for instance %r of verifier %r "
                      "(holder: %s): relaunch" % (LEASE, doc["instance"], doc.get("verifier"),
                                                  lease and lease.get("verifier")))


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
        if leased:
            lease_update(a.verifier, drop=a.instance, launcher_done=True)
        raise


# ── doctor (read-only) ───────────────────────────────────────────────────────
def cmd_doctor(a):
    doc = load_state(a.instance)
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
    probe.reset_panel(ctx)
    reset = {"step": n, "kind": "reset_panel", "attempt": attempt, "ok": True,
             "detail": "no panel open before the proof (probe.py reset_panel)"}
    save(out, n, "reset_panel", reset, list(remote.log))
    emit(reset)
    for kind, step in steps:
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
    awake = probe.keep_awake()
    with probe.desktop_lock(out=err):
        lease_check(doc)
        probe.wait_for_display()
        try:
            probe.displays_or_blocked(probe.active_displays())
            if screen_locked():
                raise probe.EnvBlocked("the screen is locked: no panel can become active, "
                                       "so nothing can be proven until it is unlocked")
        except probe.EnvBlocked:
            if awake is not None:
                awake.terminate()
            raise
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
            if awake is not None:
                awake.terminate()
    return {"ok": 0, "fail": 1, "blocked": 3}[outcome]


# ── stop / list ──────────────────────────────────────────────────────────────
def cmd_stop(a):
    check_name(a.instance)
    check_verifier(a.verifier)
    d = inst_dir(a.instance)
    claim = d / "instance.json"
    doc = read_json(claim)
    if doc is None:
        raise HarnessError("no instance %r under %s" % (a.instance, run_root()))
    if doc.get("verifier") not in (None, a.verifier):
        raise Blocked("instance %r was launched by verifier %r, not %r: stop only your own"
                      % (a.instance, doc.get("verifier"), a.verifier))
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
    # fixtures, app log). None of it is evidence: verify_evidence.py record copied the
    # evidence into the evidence directory.
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
        emit({"lease": lease, "stale": lease_stale(lease)})
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
    s = sub.add_parser("stop")
    s.add_argument("--instance", required=True)
    s.add_argument("--verifier", required=True)
    sub.add_parser("list")
    a = p.parse_args(argv)
    fn = {"name": cmd_name, "launch": cmd_launch, "doctor": cmd_doctor, "call": cmd_call,
          "stop": cmd_stop, "list": cmd_list}[a.cmd]
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
