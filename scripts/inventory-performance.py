#!/usr/bin/env python3
"""Measures `lager tui` and `lager ls` against generated fixtures of 100 and 1,000 repositories.

Build first with `cargo build --release --locked`, then run from the repository:

    python3 scripts/inventory-performance.py [--sizes 100 1000] [--runs 20] [--events 100]

Every run uses a disposable HOME, config and PATH under a fresh temporary directory, a stub
`gh` that answers after a delay, and dead proxies, so neither the real HOME nor the network is
touched. Results print as JSON on stdout; docs/reference/performance.md records them.
"""

import argparse
import codecs
import fcntl
import json
import os
import platform
import select
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time

PROJECT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PROVIDER_DELAY = 20  # seconds; below the 30 second provider bound, above every measurement


class Screen:
    """The subset of a VT100 screen that ratatui's crossterm backend draws with."""

    def __init__(self, rows, cols):
        self.rows, self.cols = rows, cols
        self.cells = [[(" ", False)] * cols for _ in range(rows)]
        self.row = self.col = 0
        self.reversed = False
        self.pending = ""

    def feed(self, data):
        text = self.pending + data
        self.pending = ""
        i, n = 0, len(text)
        while i < n:
            ch = text[i]
            if ch == "\x1b":
                if i + 1 >= n:
                    self.pending = text[i:]
                    return
                if text[i + 1] != "[":
                    i += 2
                    continue
                j = i + 2
                while j < n and not (text[j].isalpha() or text[j] == "~"):
                    j += 1
                if j >= n:
                    self.pending = text[i:]
                    return
                self.csi(text[i + 2 : j], text[j])
                i = j + 1
                continue
            if ch == "\n":
                self.row = min(self.row + 1, self.rows - 1)
                self.col = 0
            elif ch == "\r":
                self.col = 0
            elif ch >= " " and ch != "\x7f" and self.col < self.cols:
                line = self.cells[self.row]
                line[self.col] = (ch, self.reversed)
                self.col = min(self.col + 1, self.cols - 1)
            i += 1

    def csi(self, args, command):
        if command in "Hf":
            parts = [int(p) for p in args.lstrip("?").split(";") if p.isdigit()] + [1, 1]
            self.row = min(max(parts[0], 1), self.rows) - 1
            self.col = min(max(parts[1], 1), self.cols) - 1
        elif command == "J" and args in ("", "2"):
            self.cells = [[(" ", False)] * self.cols for _ in range(self.rows)]
            self.row = self.col = 0
        elif command == "K":
            line = self.cells[self.row]
            line[self.col :] = [(" ", False)] * (self.cols - self.col)
        elif command == "m":
            for code in (args or "0").split(";"):
                if code in ("0", "27"):
                    self.reversed = False
                elif code == "7":
                    self.reversed = True

    def line(self, index):
        return "".join(ch for ch, _ in self.cells[index]).rstrip()

    def text(self):
        return "\n".join(self.line(index) for index in range(self.rows))

    def selected_row(self):
        """The first table line whose repository cell is drawn reversed."""
        for index in range(5, self.rows):
            if self.cells[index][3][1] and self.cells[index][8][1]:
                return index
        return None


class Pty:
    """A child process on a fresh pseudo-terminal that is its controlling terminal."""

    def __init__(self, command, rows, cols, env):
        self.started = time.perf_counter()
        controller, user = os.openpty()
        fcntl.ioctl(user, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))

        def controlling():
            os.setsid()
            fcntl.ioctl(0, termios.TIOCSCTTY, 0)

        self.process = subprocess.Popen(
            command,
            stdin=user,
            stdout=user,
            stderr=user,
            env=env,
            preexec_fn=controlling,
            close_fds=True,
        )
        os.close(user)
        self.fd = controller
        os.set_blocking(controller, False)
        self.screen = Screen(rows, cols)
        self.decoder = codecs.getincrementaldecoder("utf-8")("replace")

    def pump(self, timeout):
        """Reads what is available within `timeout` seconds; returns the read time or None."""
        ready, _, _ = select.select([self.fd], [], [], timeout)
        if not ready:
            return None
        try:
            data = os.read(self.fd, 1 << 16)
        except (BlockingIOError, OSError):
            return None
        stamp = time.perf_counter()
        self.screen.feed(self.decoder.decode(data))
        return stamp

    def wait_for(self, predicate, timeout):
        """Returns the time of the read after which `predicate(screen)` first held."""
        if predicate(self.screen):
            return time.perf_counter()
        deadline = time.perf_counter() + timeout
        while time.perf_counter() < deadline:
            stamp = self.pump(min(0.05, max(deadline - time.perf_counter(), 0)))
            if stamp is not None and predicate(self.screen):
                return stamp
        raise TimeoutError(f"screen never matched:\n{self.screen.text()}")

    def send(self, data):
        sent = time.perf_counter()
        os.write(self.fd, data)
        return sent

    def close(self, timeout=10):
        try:
            os.write(self.fd, b"q")
        except OSError:
            pass
        deadline = time.perf_counter() + timeout
        while self.process.poll() is None and time.perf_counter() < deadline:
            self.pump(0.05)
        if self.process.poll() is None:
            self.process.send_signal(signal.SIGKILL)
            self.process.wait()
        os.close(self.fd)
        return self.process.returncode


def stub(path, body):
    with open(path, "w") as handle:
        handle.write(body)
    os.chmod(path, 0o755)


def git(env, *args, cwd=None):
    subprocess.run(["git", "-c", "maintenance.auto=false", "-c", "gc.auto=0", *args],
                   cwd=cwd, env=env, check=True, capture_output=True)


def counts(size):
    """The fixture mix; the shares add up to `size` declared and unregistered entries. Each
    conflicting checkout also has a row of its own identity beside the declaration it blocks, so
    the inventory shows `size` plus 5% rows: 105 for 100, 1,050 for 1,000."""
    mix = {
        "declared_clean": size * 50 // 100,
        "declared_dirty": size * 10 // 100,
        "declared_missing": size * 10 // 100,
        "unregistered": size * 20 // 100,
        "conflicting": size * 5 // 100,
        "unreadable": size * 5 // 100,
    }
    mix["declared_clean"] += size - sum(mix.values())
    return mix


def make_fixture(directory, size):
    """Creates HOME, root, config and stub tools under `directory`; returns the run env."""
    home = os.path.join(directory, "home")
    root = os.path.join(home, "repos")
    tools = os.path.join(directory, "tools")
    os.makedirs(root)
    os.makedirs(tools)
    env = {
        "HOME": home,
        "PATH": tools + ":" + os.path.dirname(shutil.which("git")) + ":/usr/bin:/bin",
        "TERM": "xterm-256color",
        "LAGER_CACHE_DIR": os.path.join(directory, "cache"),
        "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_CONFIG_GLOBAL": "/dev/null",
        "GIT_TERMINAL_PROMPT": "0",
        "HTTP_PROXY": "http://127.0.0.1:1",
        "HTTPS_PROXY": "http://127.0.0.1:1",
        "ALL_PROXY": "http://127.0.0.1:1",
    }
    for name in ("pbcopy", "open"):
        stub(os.path.join(tools, name), "#!/bin/sh\nexit 0\n")
    empty = ('[{"data":{"%s":{"repositories":{"nodes":[],'
             '"pageInfo":{"hasNextPage":false,"endCursor":null}}}}}]')
    stub(os.path.join(tools, "gh"),
         "#!/bin/sh\n/bin/sleep %d\n"
         "case \"$*\" in *viewer*) echo '%s' ;; *) echo '%s' ;; esac\n"
         % (PROVIDER_DELAY, empty % "viewer", empty % "organization"))

    template = os.path.join(directory, "template")
    os.makedirs(template)
    git(env, "init", "-qb", "main", cwd=template)
    with open(os.path.join(template, "README"), "w") as handle:
        handle.write("fixture\n")
    git(env, "add", "README", cwd=template)
    git(env, "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
        "commit", "-qm", "fixture", cwd=template)

    def checkout(path, origin, dirty=False):
        shutil.copytree(template, path, symlinks=True)
        with open(os.path.join(path, ".git", "config"), "a") as handle:
            handle.write('[remote "origin"]\n\turl = %s\n' % origin)
        if dirty:
            with open(os.path.join(path, "README"), "a") as handle:
                handle.write("local change\n")

    mix = counts(size)
    declarations = []
    number = 0

    def declared(kind):
        nonlocal number
        number += 1
        name = "%s-%04d" % (kind, number)
        declarations.append("github.com/perf/" + name)
        return name, os.path.join(root, "perf", name)

    for _ in range(mix["declared_clean"]):
        name, path = declared("clean")
        checkout(path, "git@github.com:perf/%s.git" % name)
    for _ in range(mix["declared_dirty"]):
        name, path = declared("dirty")
        checkout(path, "git@github.com:perf/%s.git" % name, dirty=True)
    for _ in range(mix["declared_missing"]):
        declared("missing")
    for _ in range(mix["conflicting"]):
        name, path = declared("conflict")
        checkout(path, "git@github.com:other/%s.git" % name)
    unreadable = []
    for _ in range(mix["unreadable"]):
        name, path = declared("locked")
        checkout(path, "git@github.com:perf/%s.git" % name)
        unreadable.append(path)
    for index in range(mix["unregistered"]):
        name = "local-%04d" % (index + 1)
        checkout(os.path.join(root, "local", name), "git@github.com:local/%s.git" % name)
    for path in unreadable:
        os.chmod(path, 0)

    config = os.path.join(directory, "config.toml")
    with open(config, "w") as handle:
        handle.write('root = "repos"\n\n[providers."github.com"]\npreset = "github"\n')
        for url in declarations:
            handle.write('\n[[repositories]]\nurl = "%s"\n' % url)
    return env, config, unreadable


def nearest_rank_p95(samples):
    ordered = sorted(samples)
    rank = -(-95 * len(ordered) // 100)  # ceil(0.95 n)
    return ordered[rank - 1]


def summary(samples, target=None):
    result = {
        "n": len(samples),
        "p50_ms": round(sorted(samples)[len(samples) // 2], 1),
        "p95_ms": round(nearest_rank_p95(samples), 1),
        "max_ms": round(max(samples), 1),
    }
    if target is not None:
        result["target_ms"] = target
        result["pass"] = result["p95_ms"] <= target
    return result


def usable(screen):
    return "NORMAL ·" in screen.line(0)


def sources_done(screen):
    """Local scan finished and every Git status observed: `Local + declarations` when the scan
    read everything, `Local: incomplete` when it could not, never with a pending count."""
    return usable(screen) and local_done(screen)


def local_done(screen):
    """The source line says the local scan and every Git observation finished."""
    source = screen.line(1)
    finished = source.startswith("  Local + declarations") or (
        source.startswith("  Local: incomplete") and "scanning" not in source)
    return finished and "pending" not in source


def startup(binary, env, config, runs, rows=32, cols=132):
    first, completion = [], []
    for _ in range(runs):
        pty = Pty([binary, "--config", config, "tui"], rows, cols, env)
        try:
            first.append((pty.wait_for(usable, 30) - pty.started) * 1000)
            completion.append((pty.wait_for(sources_done, 120) - pty.started) * 1000)
        finally:
            pty.close()
    return first, completion


def search_events(pty, events, busy):
    """Types a character and erases it, timing each key until the search row shows it. Keeps
    only the samples of keys sent while `busy(screen)` held."""
    samples = []
    pty.send(b"/")
    pty.wait_for(lambda s: s.line(2).startswith("  / █"), 10)
    for index in range(events):
        expected = "  / █" if index % 2 else "  / z█"
        loaded = busy(pty.screen)
        sent = pty.send(b"\x7f" if index % 2 else b"z")
        latency = (pty.wait_for(lambda s: s.line(2).startswith(expected), 10) - sent) * 1000
        if loaded:
            samples.append(latency)
    if events % 2:
        pty.send(b"\x7f")
        pty.wait_for(lambda s: s.line(2).startswith("  / █"), 10)
    pty.send(b"\x1b")
    pty.wait_for(lambda s: "NORMAL ·" in s.line(0), 10)
    return samples


def navigation_events(pty, events, busy):
    """Moves the cursor down and up, timing each key until another row is highlighted. Keeps
    only the samples of keys sent while `busy(screen)` held."""
    samples = []
    for index in range(events):
        before = pty.screen.selected_row()
        loaded = busy(pty.screen)
        sent = pty.send(b"k" if index % 2 else b"j")
        latency = (pty.wait_for(lambda s: s.selected_row() not in (None, before), 10)
                   - sent) * 1000
        if loaded:
            samples.append(latency)
    if events % 2:
        pty.send(b"k")
    return samples


def refreshing(screen):
    """A local refresh is running: the scan or its Git observations are not finished."""
    source = screen.line(1)
    return "scanning" in source or "pending" in source


def provider_loading(screen):
    return "github.com: loading" in screen.line(1)


def provider_only(screen):
    """Local sources finished while the stub provider still holds its answer."""
    return local_done(screen) and provider_loading(screen)


def interactive(binary, env, config, events, refresh=True, rows=32, cols=132):
    """Samples per condition: only keys sent while the condition held count toward `events`."""
    results = {}
    for name, measure in (("search", search_events), ("navigation", navigation_events)):
        if refresh:
            # During a local refresh: R restarts the scan whenever the previous one finished.
            pty = Pty([binary, "--config", config, "tui"], rows, cols, env)
            try:
                pty.wait_for(sources_done, 120)
                samples, sent = [], 0
                while len(samples) < events:
                    pty.wait_for(sources_done, 120)
                    pty.send(b"R")
                    pty.wait_for(refreshing, 10)
                    samples += measure(pty, 10, refreshing)
                    sent += 10
            finally:
                pty.close()
            results[name + "_during_refresh"] = (samples[:events], sent)
        # While the stub provider holds its answer, after the local scan and Git status finished.
        pty = Pty([binary, "--config", config, "tui", "--remote"], rows, cols, env)
        try:
            pty.wait_for(lambda s: usable(s) and provider_only(s), PROVIDER_DELAY)
            samples = measure(pty, events, provider_only)
        finally:
            pty.close()
        results[name + "_during_provider"] = (samples, events)
    return results


def listing(binary, env, config, runs):
    samples, code = [], None
    for _ in range(runs):
        started = time.perf_counter()
        done = subprocess.run([binary, "--config", config, "ls"], env=env,
                              capture_output=True, stdin=subprocess.DEVNULL)
        samples.append((time.perf_counter() - started) * 1000)
        code = done.returncode
    return samples, code


def machine():
    def sysctl(name):
        try:
            return subprocess.run(["sysctl", "-n", name], capture_output=True,
                                  text=True).stdout.strip()
        except OSError:
            return None

    def sw_vers():
        try:
            return subprocess.run(["sw_vers"], capture_output=True, text=True).stdout.split()
        except OSError:
            return None

    memory = sysctl("hw.memsize")
    return {
        "cpu": sysctl("machdep.cpu.brand_string"),
        "cores": sysctl("hw.ncpu"),
        "memory_gb": int(memory) // 2**30 if memory and memory.isdigit() else None,
        "os": " ".join(sw_vers() or []) or platform.platform(),
        "python": platform.python_version(),
        "load_average": os.getloadavg(),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--binary", default=os.path.join(PROJECT, "target/release/lager"))
    parser.add_argument("--sizes", type=int, nargs="+", default=[100, 1000])
    parser.add_argument("--runs", type=int, default=20)
    parser.add_argument("--events", type=int, default=100)
    parser.add_argument("--provider-only", action="store_true",
                        help="measure input during the delayed provider only")
    args = parser.parse_args()
    if not os.access(args.binary, os.X_OK):
        sys.exit("Build first: cargo build --release --locked")
    report = {"machine": machine(), "binary": args.binary,
              "date": time.strftime("%Y-%m-%d"), "fixtures": {}}
    for size in args.sizes:
        directory = tempfile.mkdtemp(prefix="lager-performance.")
        unreadable = []
        try:
            env, config, unreadable = make_fixture(directory, size)
            result = {"mix": counts(size)}
            if not args.provider_only:
                # Lager keeps no cache between processes, so every launch is cold in the PRD's
                # sense; the cold series starts right after fixture generation, the warm series
                # after three discarded launches have warmed the OS caches.
                first, done = startup(args.binary, env, config, args.runs)
                result["startup_cold_first_frame"] = summary(first, 250)
                result["startup_cold_source_completion"] = summary(done)
                startup(args.binary, env, config, 3)
                first, done = startup(args.binary, env, config, args.runs)
                result["startup_warm_first_frame"] = summary(first, 250)
                result["startup_warm_source_completion"] = summary(done)
            for name, (samples, sent) in interactive(
                args.binary, env, config, args.events, refresh=not args.provider_only
            ).items():
                result[name] = summary(samples, 50)
                result[name]["keys_sent"] = sent
            if not args.provider_only:
                samples, code = listing(args.binary, env, config, args.runs)
                result["ls"] = summary(samples)
                result["ls"]["exit_status"] = code
            report["fixtures"][str(size)] = result
            print(f"{size} repositories measured", file=sys.stderr)
        finally:
            for path in unreadable:
                os.chmod(path, 0o755)
            shutil.rmtree(directory, ignore_errors=True)
    json.dump(report, sys.stdout, indent=2)
    print()


if __name__ == "__main__":
    main()
