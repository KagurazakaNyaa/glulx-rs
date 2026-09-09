#!/usr/bin/env python3
"""Check native desktop Glk key input and optional official line-input fixture.

Linux requirements: Python 3, Xvfb, xdotool, ImageMagick `import`, libX11,
and a built glulx-rs desktop binary. A free X display and fresh XDG directories
are allocated for each run. Screenshots, app logs and saved sessions are kept.
No story is downloaded by this tool.

Usage:
  python3 tools/check-input-ui.py --candidate target/debug/glulx-rs \
      --output /tmp/input-ui --input-feature /tmp/inputfeaturetest.ulx

The original key fixture tests all 25 named key deliveries (including both
Delete and Backspace) through X11/egui, and checks the exact persisted transcript.
The generated story also checks timer cancellation and replacement prefill.
The optional official Input Feature Test checks ROT13 output after cancellation,
retained original input, and continued editing after session restoration.
"""
import argparse
import ctypes as C
import importlib.util
import json
import os
from pathlib import Path
import re
import select
import shutil
import subprocess
import tempfile
import time


class ClientMessage(C.Structure):
    _fields_ = [('type', C.c_int), ('serial', C.c_ulong), ('send_event', C.c_int),
                ('display', C.c_void_p), ('window', C.c_ulong),
                ('message_type', C.c_ulong), ('format', C.c_int), ('data', C.c_long * 5)]


class XEvent(C.Union):
    _fields_ = [('client', ClientMessage), ('pad', C.c_long * 24)]


def close_window(display_name, window):
    """Send a real WM_DELETE_WINDOW so eframe persists the session normally."""
    lib = C.CDLL('libX11.so.6')
    lib.XOpenDisplay.argtypes, lib.XOpenDisplay.restype = [C.c_char_p], C.c_void_p
    lib.XInternAtom.argtypes, lib.XInternAtom.restype = [C.c_void_p, C.c_char_p, C.c_int], C.c_ulong
    lib.XSendEvent.argtypes = [C.c_void_p, C.c_ulong, C.c_int, C.c_long, C.POINTER(XEvent)]
    lib.XFlush.argtypes = lib.XCloseDisplay.argtypes = [C.c_void_p]
    display = lib.XOpenDisplay(display_name.encode())
    assert display, f'Could not open {display_name}'
    try:
        event = XEvent()
        event.client = ClientMessage(33, 0, 1, display, window,
                                     lib.XInternAtom(display, b'WM_PROTOCOLS', 0), 32,
                                     (C.c_long * 5)(lib.XInternAtom(display, b'WM_DELETE_WINDOW', 0), 0, 0, 0, 0))
        lib.XSendEvent(display, window, 0, 0, C.byref(event))
        lib.XFlush(display)
    finally:
        lib.XCloseDisplay(display)


def decode_ron_string(encoded):
    # RON uses Rust escapes (including escaped apostrophes and \u{...}),
    # whereas Python's JSON parser only accepts the JSON subset.
    replacements = {"'": "'", '"': '"', "\\": "\\", 'n': '\n', 'r': '\r', 't': '\t', '0': '\0'}

    def unescape(match):
        value = match[1]
        if value.startswith('u{'):
            return chr(int(value[2:-1], 16))
        if value.startswith('x'):
            return chr(int(value[1:], 16))
        return replacements[value]
    assert encoded.startswith('"') and encoded.endswith('"')
    return re.sub(r"\\(u\{[0-9a-fA-F]+\}|x[0-9a-fA-F]{2}|.)", unescape, encoded[1:-1])


def load_session(directory):
    raw = (directory / 'data/glulxplayer/app.ron').read_text()
    string = r'"(?:[^"\\]|\\.)*"'
    match = re.search(r'"glulx-session-v1":\s*(' + string + ')', raw)
    assert match, 'No saved session in app.ron'
    session = decode_ron_string(match[1])
    assert session, 'The story unexpectedly halted, leaving no session'
    result = {}
    for field in ['transcript', 'input']:
        match = re.search(r'(?:^|,)' + field + r':\s*(' + string + ')', session)
        assert match, f'No {field} in saved session'
        result[field] = decode_ron_string(match[1])
    return result


def run_story(candidate, story, directory, display_name, exercise, phase='app', focus_input=False, diagnostics=False):
    directory.mkdir(exist_ok=True)
    env = os.environ.copy()
    env.update(DISPLAY=display_name, XDG_CONFIG_HOME=str(directory / 'config'),
               XDG_DATA_HOME=str(directory / 'data'), XDG_CACHE_HOME=str(directory / 'cache'),
               LIBGL_ALWAYS_SOFTWARE='1')

    def keys(*arguments):
        return subprocess.run(['xdotool', *arguments], env=env, check=True, timeout=10, capture_output=True, text=True)

    with (directory / f'{phase}.log').open('w') as log:
        # Keep executable-adjacent portable settings isolated between fixtures.
        local_candidate = directory / candidate.name
        shutil.copy2(candidate, local_candidate)
        settings_path = directory / 'glulx-settings.json'
        if not settings_path.exists():
            settings_path.write_text(json.dumps({'language': 'en'}))
        command = [str(local_candidate)] + ([] if story is None else [str(story)])
        if diagnostics:
            command += ['--diagnostics', str(directory / 'diagnostic.log')]
        app = subprocess.Popen(command, env=env, stdout=log, stderr=log)
        try:
            deadline = time.monotonic() + 30
            window = None
            while time.monotonic() < deadline:
                assert app.poll() is None, (directory / f'{phase}.log').read_text()
                result = subprocess.run(['xdotool', 'search', '--onlyvisible', '--name', '^Glulx Player$'],
                                        env=env, capture_output=True, timeout=5)
                if result.returncode == 0 and result.stdout.strip():
                    window = int(result.stdout.splitlines()[0])
                    break
                time.sleep(.1)
            assert window, 'Desktop window did not appear'
            focus_window = window
            if focus_input:
                result = subprocess.run(['xdotool', 'search', '--onlyvisible', '--name', 'Log and input$'],
                                        env=env, capture_output=True, check=True, timeout=5)
                focus_window = int(result.stdout.splitlines()[0])
            keys('windowraise', str(focus_window))
            keys('windowfocus', '--sync', str(focus_window))
            # Allow fonts/story initialization and the first Glk select.
            time.sleep(3)
            exercise(keys)
            subprocess.run(['import', '-window', 'root', str(directory / f'{phase}.png')],
                           env=env, check=True, timeout=10)
            close_window(display_name, window)
            assert app.wait(timeout=15) == 0, (directory / f'{phase}.log').read_text()
        finally:
            if app.poll() is None:
                app.kill()
                app.wait()
    return load_session(directory)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--candidate', type=Path, default=Path('target/debug/glulx-rs'))
    parser.add_argument('--input-feature', type=Path, help='Official inputfeaturetest.ulx (optional)')
    parser.add_argument('--output', type=Path, help='Parent directory for retained artifacts')
    args = parser.parse_args()
    for executable in ['Xvfb', 'xdotool', 'import']:
        assert shutil.which(executable), f'Required executable missing: {executable}'
    candidate = args.candidate.resolve()
    assert candidate.is_file(), f'Build the candidate first: {candidate}'
    if args.input_feature:
        args.input_feature = args.input_feature.resolve()
        assert args.input_feature.is_file(), args.input_feature
    if args.output:
        args.output.mkdir(parents=True, exist_ok=True)
    root = Path(tempfile.mkdtemp(prefix='glulx-input-', dir=args.output)).resolve()
    print(f'Artifacts: {root}', flush=True)
    source = Path(__file__).with_name('make-input-fixture.py')
    spec = importlib.util.spec_from_file_location('input_fixture', source)
    fixture = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(fixture)
    story = root / 'input-keys.ulx'
    story.write_bytes(fixture.story())

    read_fd, write_fd = os.pipe()
    with (root / 'xvfb.log').open('w') as log:
        xserver = subprocess.Popen(['Xvfb', '-noreset', '-displayfd', str(write_fd), '-screen', '0', '1280x900x24'],
                                   pass_fds=(write_fd,), stdout=log, stderr=log)
        os.close(write_fd)
        try:
            assert select.select([read_fd], [], [], 10)[0], 'Xvfb did not report a free display'
            display_name = ':' + os.read(read_fd, 100).decode().strip()
            assert display_name[1:].isdigit(), (root / 'xvfb.log').read_text()
            os.close(read_fd)

            def key_matrix(keys):
                for key, _ in fixture.KEYS:
                    keys('key', key)
                    time.sleep(.25)
                time.sleep(.3)
                keys('type', '--clearmodifiers', 'abc')
                time.sleep(2.2)
            session = run_story(candidate, story, root / 'keys', display_name, key_matrix, focus_input=True)
            got = [int(value) for value in re.findall(r'KEY:(-?\d+)', session['transcript'])]
            expected = [value for _, value in fixture.KEYS]
            assert got == expected, (got, expected)
            assert 'All character keys received.' in session['transcript']
            assert 'CANCELLED:3 COMPOSED:abc' in session['transcript'], session['transcript']
            assert session['input'] == 'NEW', session['input']
            print(f'PASS native special-key input: {len(expected)} exact events; timer cancellation and replacement prefill', flush=True)

            if args.input_feature:
                def interrupt_line(keys):
                    keys('type', '--clearmodifiers', 'noecho')
                    keys('key', 'Return')
                    time.sleep(.3)
                    keys('type', '--clearmodifiers', 'interrupt')
                    keys('key', 'Return')
                    time.sleep(.2)
                    keys('type', '--clearmodifiers', 'abcdef')
                    time.sleep(2.8)
                session = run_story(candidate, args.input_feature, root / 'input-feature', display_name, interrupt_line, focus_input=True)
                assert 'nopqrs' in session['transcript'] and 'rot13' in session['transcript']
                assert session['input'] == 'abcdef', session['input']
                def continue_line(keys):
                    keys('key', 'End')
                    keys('type', '--clearmodifiers', 'xyz')
                    time.sleep(.3)
                resumed = run_story(candidate, None, root / 'input-feature', display_name, continue_line, 'resume', focus_input=True)
                assert resumed['input'] == 'abcdefxyz', resumed['input']
                print('PASS official Input Feature Test: timer cancellation, ROT13 display and resumed editing', flush=True)
        finally:
            xserver.terminate()
            xserver.wait(timeout=10)


if __name__ == '__main__':
    main()
