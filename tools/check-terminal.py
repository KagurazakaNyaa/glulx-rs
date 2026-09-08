#!/usr/bin/env python3
"""Exercise the terminal host through a real Unix PTY, with original stories.

Checks direct special keys, grid/multiple-window input, prefill editing, timed
cancellation, resize events, pipe output and terminal restoration on every exit.
Requires Python 3 and a built glulx-rs; no story downloads or GUI are needed.
"""
import argparse
import codecs
import fcntl
import importlib.util
import os
from pathlib import Path
import pty
import re
import select
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time


def module(name, filename):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(filename))
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


reference = module("reference", "check-reference.py")
keys_fixture = module("keys_fixture", "make-input-fixture.py")


def terminal_configuration(attributes, platform=sys.platform):
    configuration = list(attributes)
    if platform == "darwin":
        # XNU sets PENDIN when ICANON is restored; this is pending-input state,
        # not a configuration error. Preserve strict checks of every other bit,
        # control character and speed. See apple-oss-distributions/xnu bsd/kern/tty.c.
        configuration[3] &= ~termios.PENDIN
    return configuration


class Screen:
    """Read the cursor/clear commands emitted by this TUI; keep the visible screen."""
    def __init__(self, rows=30, columns=90):
        self.rows, self.columns = rows, columns
        self.cells = [[" "] * columns for _ in range(rows)]
        self.x = self.y = 0
        self.pending = ""

    def feed(self, data):
        self.pending += data
        while self.pending:
            if self.pending.startswith("\x1b"):
                match = re.match(r"\x1b\[([0-9;?]*)([A-Za-z~])", self.pending)
                if match is None:
                    if len(self.pending) < 30:
                        return
                    raise AssertionError(f"Unknown terminal escape {self.pending[:30]!r}")
                arguments, action = match.groups()
                if action in ("H", "f"):
                    coordinates = [int(value or 1) for value in arguments.split(";")]
                    self.y, self.x = coordinates[0] - 1, (coordinates[1] if len(coordinates) > 1 else 1) - 1
                elif action == "J" and arguments == "2":
                    self.cells = [[" "] * self.columns for _ in range(self.rows)]
                self.pending = self.pending[len(match[0]):]
                continue
            character, self.pending = self.pending[0], self.pending[1:]
            if character == "\r":
                self.x = 0
            elif character == "\n":
                self.y += 1
            elif 0 <= self.y < self.rows and 0 <= self.x < self.columns:
                self.cells[self.y][self.x] = character
                self.x += 1

    def text(self):
        return "\n".join("".join(row).rstrip() for row in self.cells)


class Terminal:
    def __init__(self, candidate, story, directory):
        self.master, self.slave = pty.openpty()
        self.original = termios.tcgetattr(self.slave)
        self.resize(90, 30)
        self.screen = Screen()
        self.raw = bytearray()
        self.decoder = codecs.getincrementaldecoder("utf-8")("replace")
        self.log = directory / f"{story.stem}.pty"
        self.result_reader, result_writer = os.pipe()
        ack_reader, self.ack_writer = os.pipe()
        self.returncode = None

        # Keep the PTY session leader alive until the parent has inspected the
        # application's restored terminal state. macOS revokes the slave when
        # its session leader exits, even if another process still holds it open.
        supervisor = r'''
import os, signal, subprocess, sys
result_fd, ack_fd = map(int, sys.argv[1:3])
child = subprocess.Popen(sys.argv[3:])
signal.signal(signal.SIGWINCH, lambda signum, frame: child.send_signal(signum))
code = child.wait()
os.write(result_fd, (str(code) + "\n").encode())
os.close(result_fd)
os.read(ack_fd, 1)
os.close(ack_fd)
sys.exit(code)
'''

        def controlling_terminal():
            os.setsid()
            fcntl.ioctl(self.slave, termios.TIOCSCTTY, 0)

        self.process = subprocess.Popen(
            [sys.executable, "-c", supervisor, str(result_writer), str(ack_reader),
             str(candidate), "--headless", "--no-auto-resources", str(story)],
            stdin=self.slave, stdout=self.slave, stderr=self.slave,
            pass_fds=(result_writer, ack_reader),
            preexec_fn=controlling_terminal, env={**os.environ, "TERM": "xterm-256color"},
        )
        os.close(result_writer)
        os.close(ack_reader)

    def poll_returncode(self):
        if self.returncode is None and select.select([self.result_reader], [], [], 0)[0]:
            result = os.read(self.result_reader, 100)
            if result:
                self.returncode = int(result)
        return self.returncode if self.returncode is not None else self.process.poll()

    def resize(self, columns, rows):
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack("HHHH", rows, columns, 0, 0))
        if hasattr(self, "process"):
            self.screen = Screen(rows, columns)
            os.kill(self.process.pid, signal.SIGWINCH)

    def pump(self, timeout=.05):
        if select.select([self.master], [], [], timeout)[0]:
            data = os.read(self.master, 65536)
            self.raw.extend(data)
            self.screen.feed(self.decoder.decode(data))

    def wait_for(self, text, timeout=10):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            self.pump()
            if text in self.screen.text():
                return
            assert self.poll_returncode() is None, self.raw.decode(errors="replace")
        raise AssertionError(f"Did not display {text!r}:\n{self.screen.text()}")

    def send(self, data):
        os.write(self.master, data.encode() if isinstance(data, str) else data)

    def finish(self, expected=0):
        deadline = time.monotonic() + 10
        while self.poll_returncode() is None and time.monotonic() < deadline:
            self.pump()
        assert self.poll_returncode() == expected, (self.poll_returncode(), self.screen.text())
        self.pump(.05)
        restored = termios.tcgetattr(self.slave)
        assert terminal_configuration(restored) == terminal_configuration(self.original), (
            f"TTY attributes were not restored: original={self.original!r}, restored={restored!r}, "
            f"lflag_delta={self.original[3] ^ restored[3]:#x}, PENDIN={getattr(termios, 'PENDIN', 0):#x}"
        )
        assert b"\x1b[?1049l" in self.raw, "Alternate screen was not restored"
        assert b"\x1b[?25h" in self.raw, "Cursor was not restored"
        os.write(self.ack_writer, b"1")
        assert self.process.wait(timeout=5) == expected

    def close(self):
        if self.process.poll() is None:
            os.killpg(self.process.pid, signal.SIGKILL)
            self.process.wait()
        self.log.write_bytes(self.raw)
        os.close(self.master)
        os.close(self.slave)
        os.close(self.result_reader)
        os.close(self.ack_writer)


def builder():
    story = reference.StoryBuilder()
    story.instruction(0x149, 2, 0)
    return story


def wait_event(b, kind, label):
    mem = lambda address: (7, address)
    b.labels[label] = len(b.code)
    b.glk(0xc0, [0x8020])
    b.instruction(0x25, mem(0x8020), kind, label)


def windows_story():
    b = builder()
    mem = lambda address: (7, address)
    b.glk(0x23, [0, 0, 0, 3, 0], mem(0x8000))
    b.glk(0x23, [mem(0x8000), 0x12, 3, 4, 0], mem(0x8004))
    b.glk(0x23, [mem(0x8000), 0x13, 4, 3, 0], mem(0x8008))
    b.glk(0x2f, [mem(0x8004)])
    b.text("TTY GRID: alive\nScore: 7\nGRID> ")
    b.glk(0x2f, [mem(0x8008)])
    b.text("SECOND WINDOW\n")
    b.glk(0x2f, [mem(0x8000)])
    b.text("GRID EDIT READY\n")
    for index, character in enumerate("ab"):
        b.instruction(0x42, ord(character), mem(0x8100 + index))
    b.glk(0xd0, [mem(0x8004), 0x8100, 8, 2])
    wait_event(b, 3, "grid")
    b.text("GRID INPUT:")
    b.glk(0x84, [0x8100, mem(0x8028)])
    b.text("\nMULTI READY\n")
    b.instruction(0x42, ord('P'), mem(0x8100))
    b.instruction(0x42, ord('Q'), mem(0x8200))
    b.glk(0xd0, [mem(0x8000), 0x8100, 30, 1])
    b.glk(0xd0, [mem(0x8008), 0x8200, 30, 1])
    wait_event(b, 3, "second")
    b.text("SECOND INPUT:")
    b.glk(0x84, [0x8200, mem(0x8028)])
    b.text("\n")
    wait_event(b, 3, "first")
    b.text("FIRST INPUT:")
    b.glk(0x84, [0x8100, mem(0x8028)])
    b.text("\nRESIZE READY\n")
    b.glk(0x24, [mem(0x8008), 0])
    wait_event(b, 5, "resize")
    b.glk(0x25, [mem(0x8004), 0x8040, 0])
    b.text("RESIZE:")
    b.instruction(0x71, mem(0x8040))
    b.text("\nNOECHO READY\n")
    b.glk(0x150, [mem(0x8000), 0])
    b.instruction(0x40, 0xfffffff8, mem(0x8050))
    b.glk(0x151, [mem(0x8000), 0x8050, 1])
    b.glk(0xd0, [mem(0x8000), 0x8100, 40, 0])
    wait_event(b, 3, "terminator")
    b.text("TERMINATOR:")
    b.instruction(0x71, mem(0x802c))
    b.text(" LENGTH:")
    b.instruction(0x71, mem(0x8028))
    b.glk(0x150, [mem(0x8000), 1])
    b.glk(0x151, [mem(0x8000), 0, 0])
    b.text("\nFINAL READY\n")
    for index, character in enumerate("NEW"):
        b.instruction(0x42, ord(character), mem(0x8100 + index))
    b.glk(0xd0, [mem(0x8000), 0x8100, 40, 3])
    wait_event(b, 3, "last")
    b.text("FINAL:")
    b.glk(0x84, [0x8100, mem(0x8028)])
    b.text("\nTTY COMPLETE\n")
    b.instruction(0x120)
    return b.finish(0x8000, 0x9000, 0x9100)


def simple_story(error=False):
    b = builder()
    mem = lambda address: (7, address)
    b.glk(0x23, [0, 0, 0, 3, 0], mem(0x8000))
    b.glk(0x2f, [mem(0x8000)])
    b.text("PIPE READY\n")
    if error:
        b.instruction(0x7f)
    else:
        b.glk(0xd0, [mem(0x8000), 0x8100, 40, 0])
        wait_event(b, 3, "input")
        b.text("PIPE:")
        b.glk(0x84, [0x8100, mem(0x8028)])
        b.text("\n")
        b.instruction(0x120)
    return b.finish(0x8000, 0x9000, 0x9100)


def files_story():
    b = builder()
    mem = lambda address: (7, address)
    b.glk(0x23, [0, 0, 0, 3, 0], mem(0x8000))
    b.glk(0x2f, [mem(0x8000)])
    b.text("FILE READY\n")
    b.glk(0x62, [0, 1, 0], mem(0x8004))
    b.glk(0x42, [mem(0x8004), 1, 0], mem(0x8008))
    b.glk(0x47, [mem(0x8008)])
    b.text("TTY FILE DATA")
    b.glk(0x44, [mem(0x8008), 0])
    b.glk(0x2f, [mem(0x8000)])
    b.text("FILE WRITTEN\nCANCEL FILE READY\n")
    b.glk(0x62, [0, 2, 0], mem(0x8004))
    b.text("CANCELLED FILE:")
    b.instruction(0x71, mem(0x8004))
    b.text("\n")
    b.instruction(0x120)
    return b.finish(0x8000, 0x9000, 0x9100)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--candidate", type=Path, default=Path("target/debug/glulx-rs"))
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    candidate = args.candidate.resolve()
    assert candidate.is_file(), f"Build first: {candidate}"
    if args.output:
        args.output.mkdir(parents=True, exist_ok=True)
    directory = Path(tempfile.mkdtemp(prefix="glulx-terminal-", dir=args.output))
    print(f"Artifacts: {directory}", flush=True)
    stories = {"keys": keys_fixture.story(), "windows": windows_story(), "files": files_story(), "pipe": simple_story(), "error": simple_story(True)}
    for name, data in stories.items():
        (directory / f"{name}.ulx").write_bytes(data)

    escape_keys = [b"\x1b[D", b"\x1b[C", b"\x1b[A", b"\x1b[B", b"\x1b[3~", b"\x7f", b"\x1b", b"\t", b"\x1b[5~", b"\x1b[6~", b"\x1b[H", b"\x1b[F"]
    escape_keys += [b"\x1bOP", b"\x1bOQ", b"\x1bOR", b"\x1bOS"]
    escape_keys += [f"\x1b[{number}~".encode() for number in [15, 17, 18, 19, 20, 21, 23, 24]] + [b"\r"]
    terminal = Terminal(candidate, directory / "keys.ulx", directory)
    try:
        for (name, code), sequence in zip(keys_fixture.KEYS, escape_keys):
            terminal.wait_for(f"Press {name}:")
            terminal.send(sequence)
            terminal.wait_for(f"KEY:{code}")
        terminal.wait_for("Type abc before the timer expires.")
        terminal.send("abc")
        terminal.wait_for("CANCELLED:3 COMPOSED:abc")
        terminal.wait_for("NEW")
        terminal.send(b"\x03")
        terminal.finish()
        print("PASS 25 immediate special keys; live timed cancellation; replacement prefill; Ctrl+C restoration", flush=True)
    finally:
        terminal.close()

    terminal = Terminal(candidate, directory / "windows.ulx", directory)
    try:
        terminal.wait_for("GRID> ab")
        terminal.wait_for("SECOND WINDOW")
        terminal.send(b"\x1b[DX\x1b[F!\r")
        terminal.wait_for("GRID INPUT:aXb!")
        terminal.wait_for("MULTI READY")
        terminal.send(b"\x0ez\r")
        terminal.wait_for("SECOND INPUT:Qz")
        terminal.send("q\r")
        terminal.wait_for("FIRST INPUT:Pq")
        terminal.wait_for("RESIZE READY")
        assert "SECOND WINDOW" not in terminal.screen.text(), terminal.screen.text()
        terminal.resize(65, 27)
        terminal.wait_for("RESIZE:65")
        terminal.wait_for("NOECHO READY")
        terminal.send("mask\x1b")
        terminal.wait_for("TERMINATOR:-8 LENGTH:4")
        assert "mask" not in terminal.screen.text(), terminal.screen.text()
        terminal.wait_for("FINAL READY")
        terminal.send("!\r")
        terminal.finish()
        assert b"FINAL:NEW!" in terminal.raw
        assert b"TTY COMPLETE" in terminal.raw
        print("PASS grid rendering/editing; concurrent windows; close; resize/Arrange; echo control/terminator; normal-exit restoration", flush=True)
    finally:
        terminal.close()

    terminal = Terminal(candidate, directory / "files.ulx", directory)
    try:
        terminal.wait_for("FILE READY")
        output = directory / "written.glkdata"
        terminal.send(str(output) + "\r")
        terminal.wait_for("CANCEL FILE READY")
        terminal.send(b"\x1b")
        terminal.finish()
        assert output.read_bytes() == b"TTY FILE DATA"
        assert b"CANCELLED FILE:0" in terminal.raw
        print("PASS terminal file-path editor, exact saved bytes and Escape cancellation", flush=True)
    finally:
        terminal.close()

    for name, expected in [("pipe", 0), ("error", 1)]:
        terminal = Terminal(candidate, directory / f"{name}.ulx", directory)
        try:
            if name == "pipe":
                terminal.wait_for("PIPE READY")
                terminal.send(b"\x04")
            terminal.finish(expected)
        finally:
            terminal.close()
    result = subprocess.run([str(candidate), "--headless", str(directory / "pipe.ulx")], input="hello\n", text=True, capture_output=True, check=True, timeout=10)
    assert result.stdout == "PIPE READY\nPIPE:hello\n", repr(result.stdout)
    print("PASS EOF/error restoration; exact redirected pipe output", flush=True)


if __name__ == "__main__":
    main()
