"""Unit test of plugin/scripts/lib/dcv2_portable.py (no Claude call). Run by plugin_test.sh.

Windows branches are never faked on another OS: the pure selectors and the pure Windows path functions (over an
injected `ntpath`, a fake cygpath and an identity realpath) are tested everywhere; the end-to-end `nt` cases run
only where os.name == "nt" and print a visible skip elsewhere.
"""
import ntpath
import os
import re
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
LIB = os.path.join(ROOT, "plugin", "scripts", "lib")
sys.dont_write_bytecode = True  # the plugin folder never gets a __pycache__
sys.path.insert(0, LIB)
import dcv2_portable as p  # noqa: E402


def read(path):
    with open(path, encoding="utf-8") as f:
        return f.read()


def skip_unless_nt(test):
    if os.name != "nt":
        print(f"skip: {test.__name__} runs only where os.name == 'nt' (this is {os.name!r})")
        return unittest.skip("Windows only")(test)
    return test


class Backend(unittest.TestCase):
    def test_selector_picks_each_branch(self):
        self.assertEqual(p._backend("posix").name, "posix")
        self.assertEqual(p._backend("nt").name, "nt")
        self.assertEqual(p._backend("java").name, "posix")
        self.assertEqual(p._backend("posix").spawn_kwargs(), {"start_new_session": True})
        self.assertEqual(p._backend("nt").spawn_kwargs(), {"creationflags": 0x08000200})

    def test_check_argv_uses_the_passed_bash_only_on_windows(self):
        env = {"DCV2_BASH_EXE": r"C:\Program Files\Git\bin\bash.exe"}
        self.assertEqual(p.check_argv("npm test", "nt", env), [r"C:\Program Files\Git\bin\bash.exe", "-c", "npm test"])
        self.assertEqual(p.check_argv("npm test", "posix", env), ["bash", "-c", "npm test"])
        with self.assertRaises(FileNotFoundError):
            p.check_argv("x", "nt", {})
        with self.assertRaises(FileNotFoundError):
            p.check_argv("x", "nt", {"DCV2_BASH_EXE": "bash"})


@unittest.skipIf(os.name == "nt", "POSIX lock")
class PosixLock(unittest.TestCase):
    def test_a_second_holder_times_out_with_the_exact_message(self):
        with tempfile.TemporaryDirectory() as d:
            path = os.path.join(d, ".lock")
            first = open(path, "a+")
            second = open(path, "a+")
            p.lock(first)
            clock = iter([0.0, 0.1, 0.2, 25.0])
            with self.assertRaises(p.LockBusy) as ctx:
                p.lock(second, timeout=25, sleep=lambda s: None, clock=lambda: next(clock))
            self.assertEqual(str(ctx.exception), "memory lock busy: another memory.sh is still running (waited 25 s)")
            p.unlock(first)
            p.lock(second, timeout=1)  # free now
            p.unlock(second)
            first.close()
            second.close()

    def test_a_short_real_bound_ends_in_time(self):
        with tempfile.TemporaryDirectory() as d:
            path = os.path.join(d, ".lock")
            first, second = open(path, "a+"), open(path, "a+")
            p.lock(first)
            with self.assertRaises(p.LockBusy) as ctx:
                p.lock(second, timeout=0.2)
            self.assertEqual(str(ctx.exception), "memory lock busy: another memory.sh is still running (waited 0.2 s)")
            first.close()
            second.close()


class NtOnly(unittest.TestCase):
    @skip_unless_nt
    def test_lock_contention_on_a_non_empty_a_plus_file(self):
        with tempfile.TemporaryDirectory() as d:
            path = os.path.join(d, ".lock")
            with open(path, "w") as f:
                f.write("existing content")
            a, b = open(path, "a+"), open(path, "a+")
            p.lock(a)
            with self.assertRaises(p.LockBusy):
                p.lock(b, timeout=0.2)
            p.unlock(a)
            a.close()
            b.close()

    @skip_unless_nt
    def test_a_timed_out_grandchild_is_killed(self):
        proc = p.spawn_group(["cmd", "/C", "ping -n 30 127.0.0.1 > NUL"], stdout=subprocess.DEVNULL)
        p.kill_group(proc)
        self.assertIsNotNone(proc.wait(timeout=10))


class Replace(unittest.TestCase):
    def test_retries_a_transient_error_on_windows(self):
        calls = []

        def flaky(a, b):
            calls.append((a, b))
            if len(calls) < 4:
                raise PermissionError(13, "sharing violation")

        slept = []
        p.replace("t", "d", replace_fn=flaky, nt=True, sleep=slept.append)
        self.assertEqual(len(calls), 4)
        self.assertEqual(slept, [0.03] * 3)

    def test_gives_up_after_ten_tries_with_the_last_error(self):
        calls = []

        def always(a, b):
            calls.append(1)
            raise PermissionError(13, "denied")

        with self.assertRaises(PermissionError):
            p.replace("t", "d", replace_fn=always, nt=True, sleep=lambda s: None)
        self.assertEqual(len(calls), 10)

    def test_posix_is_a_plain_replace(self):
        calls = []

        def always(a, b):
            calls.append(1)
            raise PermissionError(13, "denied")

        with self.assertRaises(PermissionError):
            p.replace("t", "d", replace_fn=always, nt=False, sleep=lambda s: None)
        self.assertEqual(calls, [1])

    def test_a_non_permission_error_is_not_retried(self):
        calls = []

        def missing(a, b):
            calls.append(1)
            raise FileNotFoundError(2, "gone")

        with self.assertRaises(FileNotFoundError):
            p.replace("t", "d", replace_fn=missing, nt=True, sleep=lambda s: None)
        self.assertEqual(calls, [1])

    def test_a_real_replace_works(self):
        with tempfile.TemporaryDirectory() as d:
            a, b = os.path.join(d, "a"), os.path.join(d, "b")
            for path, text in ((a, "new"), (b, "old")):
                with open(path, "w") as f:
                    f.write(text)
            p.replace(a, b)
            self.assertEqual(read(b), "new")


def fake_cygpath(path):
    """/work/x -> C:\\msys\\work\\x, like Git Bash's cygpath -w for a path under its root; /bad fails."""
    if path.startswith("/bad"):
        return None
    return "C:\\msys" + path.replace("/", "\\")


def identity_realpath(path):
    return path


class NtPaths(unittest.TestCase):
    def ident(self, value):
        return p.nt_ident(value, ntpath, fake_cygpath, identity_realpath)

    def test_the_four_spellings_are_one_identity(self):
        spellings = [r"C:\a\b", "C:/a/b", "/c/a/b", "/C/A/B"]
        self.assertEqual(len({self.ident(s) for s in spellings}), 1)
        self.assertEqual(self.ident("/c/a/b"), r"c:\a\b")

    def test_the_null_device_and_failures(self):
        self.assertIsNone(self.ident("/dev/null"))
        self.assertIsNone(self.ident("NUL"))
        self.assertEqual(self.ident("/bad/path"), p.UNRESOLVED)
        self.assertEqual(self.ident("/work/x"), r"c:\msys\work\x")

    def test_real_path_keeps_the_case(self):
        real = lambda v: p.nt_real_path(v, ntpath, fake_cygpath, identity_realpath)  # noqa: E731
        self.assertEqual(real("/c/Users/Ab"), r"C:\Users\Ab")
        self.assertEqual(real("C:/Users/Ab"), "C:/Users/Ab")  # realpath is injected as identity here
        self.assertEqual(real("/bad/Ab"), "/bad/Ab")

    def test_real_path_with_the_real_ntpath_realpath_normalises_slashes(self):
        real = lambda v: p.nt_real_path(v, ntpath, fake_cygpath, ntpath.normpath)  # noqa: E731
        self.assertEqual(real("C:/Users/Ab"), r"C:\Users\Ab")

    def test_is_protected_under_each_spelling(self):
        run = p.nt_ident(r"C:\app\runs\r-x", ntpath, fake_cygpath, identity_realpath)
        names = frozenset(["status.json", ".guard.json"])
        ident_fn = lambda v: p.nt_ident(v, ntpath, fake_cygpath, identity_realpath)  # noqa: E731
        for spelling in (r"C:\app\runs\r-x\status.json", "C:/app/runs/r-x/status.json", "/c/app/runs/r-x/status.json",
                         "/C/APP/RUNS/R-X/STATUS.JSON"):
            self.assertTrue(p.is_protected(spelling, run, names, ntpath, ident_fn), spelling)
        self.assertFalse(p.is_protected("/c/app/runs/r-x/notes.md", run, names, ntpath, ident_fn))
        self.assertFalse(p.is_protected("/c/other/status.json", run, names, ntpath, ident_fn))
        self.assertTrue(p.is_protected("/c/app/runs/r-x/requests/1.json", run, names, ntpath, ident_fn))
        self.assertTrue(p.is_protected("/bad/status.json", run, names, ntpath, ident_fn), "UNRESOLVED is refused")
        self.assertFalse(p.is_protected("/dev/null", run, names, ntpath, ident_fn), "the null device is no file")


class PosixIdentity(unittest.TestCase):
    @unittest.skipIf(os.name == "nt", "POSIX identity")
    def test_ident_and_real_path_are_exactly_today(self):
        with tempfile.TemporaryDirectory() as d:
            mixed = os.path.join(d, "RepoX", "MyApp")
            os.makedirs(mixed)
            link = os.path.join(d, "Link")
            os.symlink(mixed, link)
            for value in (d, mixed, link, os.path.join(link, "..", "x"), "relative/Path", "/", "/nonexistent/AbC", "~"):
                self.assertEqual(p.ident(value), os.path.realpath(value).casefold(), value)
                self.assertEqual(p.real_path(value), os.path.realpath(value), value)
            self.assertIn("RepoX", p.real_path(mixed))  # the case is kept
            self.assertNotIn("RepoX", p.ident(mixed))


class Scripts(unittest.TestCase):
    def test_every_importing_script_sets_dont_write_bytecode_first(self):
        found = 0
        scripts = os.path.join(ROOT, "plugin", "scripts")
        for name in sorted(os.listdir(scripts)):
            if not name.endswith(".sh"):
                continue
            text = read(os.path.join(scripts, name))
            if "import dcv2_portable" in text:
                found += 1
                self.assertRegex(text, r"sys\.dont_write_bytecode = True[^\n]*\n(?:[^\n]*\n)?sys\.path\.insert\(0, os\.environ\[\"DCV2_LIB_DIR\"\]\)",
                                 name)
                self.assertLess(text.index("sys.dont_write_bytecode = True"), text.index("import dcv2_portable"), name)
        self.assertGreaterEqual(found, 3)

    def test_no_script_keeps_its_own_lock_kill_or_replace(self):
        scripts = os.path.join(ROOT, "plugin", "scripts")
        for name in sorted(os.listdir(scripts)):
            if not name.endswith(".sh"):
                continue
            for number, line in enumerate(read(os.path.join(scripts, name)).splitlines(), 1):
                if line.lstrip().startswith("#"):
                    continue
                for needle in ("import fcntl", "os.killpg", "start_new_session", "os.replace("):
                    self.assertNotIn(needle, line, f"{name}:{number}")


if __name__ == "__main__":
    unittest.main(verbosity=1)
