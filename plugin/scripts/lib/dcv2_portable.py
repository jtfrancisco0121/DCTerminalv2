"""The plugin's portable helpers for embedded Python (stdlib only, Python 3.8+).

guard.sh, memory.sh, state.sh and stop-check.sh put their own `scripts/lib` first on sys.path (from their own path,
never from a user-settable variable), set `sys.dont_write_bytecode = True` and import this module. Every OS
difference of the plugin's Python lives here (docs/launch.md, docs/run-folder.md):

- `lock` / `unlock`: an exclusive file lock with a bound (POSIX `fcntl.flock`, Windows `msvcrt.locking`).
- `spawn_group` / `kill_group`: a child in its own group, killed with its descendants.
- `ident(p)`: the identity of a path, for comparisons only. `real_path(p)`: the path to open, store, print or use as a
  cwd. Never pass an `ident` result to open(), os.path.join, cwd=, print or a JSON write.
- `replace`: an atomic replace that retries a transient sharing error on Windows.
- `check_argv`: the argv of a repo check (an absolute Git Bash on Windows, `bash` elsewhere).

Each OS branch is a small backend chosen by `_backend(os_name)`, so both branches are unit-tested on any OS. Windows
logic over paths is written as pure functions over an injected path module (`ntpath`), `cygpath` and `realpath`.
"""
import functools
import ntpath
import os
import re
import subprocess
import time

LOCK_BOUND = 25  # seconds; below the app's 30 s memory timeout, so the app shows this script's message
LOCK_BUSY_FORMAT = "memory lock busy: another memory.sh is still running (waited %g s)"
CREATE_NO_WINDOW = 0x08000000
CREATE_NEW_PROCESS_GROUP = 0x00000200
UNRESOLVED = "\0unresolved"  # the identity of a path that could not be resolved (fail closed)


class LockBusy(Exception):
    """The lock was still held when the bound ran out."""


class _Posix:
    name = "posix"

    @staticmethod
    def try_lock(fh):
        import fcntl
        try:
            fcntl.flock(fh, fcntl.LOCK_EX | fcntl.LOCK_NB)
            return True
        except OSError:
            return False

    @staticmethod
    def unlock(fh):
        import fcntl
        fcntl.flock(fh, fcntl.LOCK_UN)

    @staticmethod
    def spawn_kwargs():
        return {"start_new_session": True}

    @staticmethod
    def kill(proc):
        import signal
        try:
            os.killpg(proc.pid, signal.SIGKILL)
        except OSError:
            pass


class _Nt:
    name = "nt"

    @staticmethod
    def try_lock(fh):
        import msvcrt
        try:
            fh.seek(0)
            msvcrt.locking(fh.fileno(), msvcrt.LK_NBLCK, 1)
            return True
        except OSError:
            return False

    @staticmethod
    def unlock(fh):
        import msvcrt
        try:
            fh.seek(0)
            msvcrt.locking(fh.fileno(), msvcrt.LK_UNLCK, 1)
        except OSError:
            pass

    @staticmethod
    def spawn_kwargs():
        return {"creationflags": CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP}

    @staticmethod
    def kill(proc):
        try:
            subprocess.run(["taskkill", "/F", "/T", "/PID", str(proc.pid)], stdout=subprocess.DEVNULL,
                           stderr=subprocess.DEVNULL, stdin=subprocess.DEVNULL, creationflags=CREATE_NO_WINDOW)
        except OSError:
            pass


def _backend(os_name):
    """The backend for an `os.name` value: a pure selector (imports happen inside the methods)."""
    return _Nt if os_name == "nt" else _Posix


def _current():
    return _backend(os.name)


def lock(fh, timeout=LOCK_BOUND, backend=None, sleep=time.sleep, clock=time.monotonic):
    """Takes an exclusive lock on `fh` (opened "a+"), retrying every 50 ms for `timeout` seconds; raises LockBusy."""
    be = backend or _current()
    deadline = clock() + timeout
    fh.seek(0)
    while True:
        if be.try_lock(fh):
            return
        if clock() >= deadline:
            raise LockBusy(LOCK_BUSY_FORMAT % timeout)
        sleep(0.05)


def unlock(fh, backend=None):
    (backend or _current()).unlock(fh)


def spawn_group(args, backend=None, **kw):
    """subprocess.Popen(args) in its own group (POSIX session / Windows process group, no console window)."""
    be = backend or _current()
    kw = dict(kw)
    kw.update(be.spawn_kwargs())
    return subprocess.Popen(args, **kw)


def kill_group(proc, backend=None):
    """Kills a spawn_group child and everything it started; never raises."""
    (backend or _current()).kill(proc)


# ---- bash for repo checks -------------------------------------------------------------------------------------------
def bash_exe(os_name=None, env=None):
    """The bash that runs a repo check: on Windows the absolute path the calling script passed in
    DCV2_BASH_EXE (its own bash, never a bare `bash`, which CreateProcess would find in System32); else `bash`.
    On Windows with DCV2_BASH_EXE unset or not an absolute path it raises FileNotFoundError (fail closed)."""
    env = os.environ if env is None else env
    if (os_name or os.name) == "nt":
        exe = env.get("DCV2_BASH_EXE") or ""
        if not ntpath.isabs(exe):
            raise FileNotFoundError("DCV2_BASH_EXE is not set to an absolute bash path; the check was not run")
        return exe
    return "bash"


def check_argv(cmd, os_name=None, env=None):
    return [bash_exe(os_name, env), "-c", cmd]


# ---- atomic replace -------------------------------------------------------------------------------------------------
def replace(tmp, dst, replace_fn=os.replace, nt=None, tries=10, wait=0.03, sleep=time.sleep):
    """os.replace(tmp, dst). On Windows a reader that has dst open (the app) makes the replace fail with a sharing
    error, so it is retried (10 x 30 ms); POSIX is a plain os.replace. After the last try the error is raised."""
    nt = (os.name == "nt") if nt is None else nt
    if not nt:
        return replace_fn(tmp, dst)
    for attempt in range(tries):
        try:
            return replace_fn(tmp, dst)
        except PermissionError:
            if attempt == tries - 1:
                raise
            sleep(wait)


# ---- path identity (AD-7) -------------------------------------------------------------------------------------------
_DRIVE_RE = re.compile(r"^/([A-Za-z])(?:/(.*))?$", re.S)


def _msys_drive(p):
    """`/c/Users/x` -> `C:\\Users\\x`; None when `p` is not a drive-letter spelling."""
    m = _DRIVE_RE.match(p)
    if not m:
        return None
    return m.group(1).upper() + ":\\" + (m.group(2) or "").replace("/", "\\")


def _is_null_device(p):
    return p == "/dev/null" or p.upper() == "NUL"


def _posixish_absolute(p):
    return p.startswith("/") and not p.startswith("//")


def nt_real_path(p, pathmod, cygpath, realpath):
    """A Windows path VALUE: `/c/x` -> `C:\\x`, other POSIX-absolute paths by cygpath, then realpath. The case is kept
    (no normcase, no casefold). A path cygpath cannot convert is returned as given."""
    if _posixish_absolute(p):
        converted = _msys_drive(p)
        if converted is None:
            converted = cygpath(p)
        if converted is None:
            return p
        p = converted
    return realpath(p)


def nt_ident(p, pathmod, cygpath, realpath):
    """A Windows path IDENTITY, for comparison only: None for the null device, UNRESOLVED when cygpath cannot convert
    it, else the real path, normcase'd and casefolded. `C:\\a\\b`, `C:/a/b`, `/c/a/b` and `/C/A/B` are one identity."""
    if _is_null_device(p):
        return None
    if _posixish_absolute(p):
        converted = _msys_drive(p)
        if converted is None:
            converted = cygpath(p)
        if converted is None:
            return UNRESOLVED
        p = converted
    return pathmod.normcase(realpath(p)).casefold()


@functools.lru_cache(maxsize=256)
def _cached_cygpath(p):
    try:
        out = subprocess.run(["cygpath", "-w", p], capture_output=True, text=True, stdin=subprocess.DEVNULL,
                             creationflags=CREATE_NO_WINDOW if os.name == "nt" else 0, timeout=10)
    except (OSError, subprocess.SubprocessError):
        return None
    text = out.stdout.strip()
    return text if out.returncode == 0 and text else None


def ident(p):
    """The identity of a path, for comparison only (`ident(a) == ident(b)`, or a prefix test between two idents).
    POSIX: exactly `os.path.realpath(p).casefold()`. Never opened, joined, stored or printed."""
    if os.name == "nt":
        import ntpath
        return nt_ident(p, ntpath, _cached_cygpath, os.path.realpath)
    return os.path.realpath(p).casefold()


def real_path(p):
    """A path VALUE that is opened, stored, printed, joined or used as a cwd. POSIX: exactly `os.path.realpath(p)`
    (no casefold: a stored or printed path keeps its case)."""
    if os.name == "nt":
        import ntpath
        return nt_real_path(p, ntpath, _cached_cygpath, os.path.realpath)
    return os.path.realpath(p)


def is_protected(p, run_ident, names_cf, pathmod, ident_fn, has_request_md=lambda d: False):
    """Whether `p` (a path value) is a protected run file of the run whose identity is `run_ident`: a name in
    `names_cf` (casefolded) directly in the run folder or in a folder holding a request.md, or anything under a
    `requests` folder of one. An unresolvable path is protected (fail closed); the null device is not a file."""
    ident_p = ident_fn(p)
    if ident_p == UNRESOLVED:
        return True
    if ident_p is None:
        return False
    q = p
    while q and q != pathmod.dirname(q):
        if pathmod.basename(q).casefold() == "requests":
            d = pathmod.dirname(q)
            d_ident = ident_fn(d)
            if d_ident == UNRESOLVED or (d_ident is not None and d_ident == run_ident) or has_request_md(d):
                return True
        q = pathmod.dirname(q)
    if pathmod.basename(p).casefold() not in names_cf:
        return False
    d = pathmod.dirname(p)
    d_ident = ident_fn(d)
    return d_ident == UNRESOLVED or (d_ident is not None and d_ident == run_ident) or has_request_md(d)
