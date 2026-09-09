#!/usr/bin/env python3
"""Check native companion-window input, resize isolation and close/reopen behavior.

Requires the same Linux/Xvfb tools as check-input-ui.py. Uses an original fixture.
"""
import argparse
import json
import importlib.util
import os
from pathlib import Path
import re
import select
import subprocess
import tempfile
import time


def module(name):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(name + '.py'))
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--candidate', type=Path, default=Path('target/release/glulx-rs'))
    parser.add_argument('--output', type=Path)
    parser.add_argument('--font-dialogs', action='store_true', help='Exercise GTK system-font and file dialogs')
    args = parser.parse_args()
    if args.output:
        args.output.mkdir(parents=True, exist_ok=True)
    root = Path(tempfile.mkdtemp(prefix='glulx-windows-', dir=args.output)).resolve()
    print(f'Artifacts: {root}', flush=True)
    harness = module('check-input-ui')
    builder = module('check-reference').StoryBuilder()
    mem = lambda address: (7, address)
    builder.instruction(0x149, 2, 0)
    builder.glk(0x23, [0, 0, 0, 3, 0], mem(0x800))
    builder.glk(0x2f, [mem(0x800)])
    builder.text('Ready for a command.\n')
    builder.labels['request'] = len(builder.code)
    builder.glk(0xd0, [mem(0x800), 0x900, 32, 0])
    builder.labels['wait'] = len(builder.code)
    builder.glk(0xc0, [0x810])
    builder.instruction(0x25, mem(0x810), 3, 'wait')
    builder.text('Received: ')
    builder.glk(0x84, [0x900, mem(0x818)])
    builder.instruction(0x70, 10)
    builder.instruction(0x20, 'request')
    story = root / 'windows.ulx'
    story.write_bytes(builder.finish(0x800, 0x1000, 0x1100))
    read_fd, write_fd = os.pipe()
    with (root / 'xvfb.log').open('w') as log:
        server = subprocess.Popen(['Xvfb', '-noreset', '-displayfd', str(write_fd), '-screen', '0', '1280x900x24'],
                                  pass_fds=(write_fd,), stdout=log, stderr=log)
        os.close(write_fd)
        try:
            assert select.select([read_fd], [], [], 10)[0]
            display = ':' + os.read(read_fd, 100).decode().strip()
            os.close(read_fd)
            env = os.environ | {'DISPLAY': display}
            def find(title):
                result = subprocess.run(['xdotool', 'search', '--onlyvisible', '--name', title],
                                        env=env, capture_output=True, text=True, timeout=5)
                return [int(value) for value in result.stdout.split()]

            def exercise(keys):
                canvas = find('^Glulx Player$')[0]
                inputs = find('Log and input$')[0]
                translation = find('Translation$')[0]
                assert len({canvas, inputs, translation}) == 3
                keys('windowraise', str(canvas))
                subprocess.run(['import', '-window', str(canvas), str(root / 'toolbar.png')], env=env, check=True, timeout=10)
                geometry = keys('getwindowgeometry', '--shell', str(canvas)).stdout
                def toolbar(x):
                    keys('windowraise', str(canvas))
                    keys('windowfocus', '--sync', str(canvas))
                    keys('mousemove', '--window', str(canvas), str(x), '34', 'click', '1')
                    time.sleep(.3)
                toolbar(185)  # Log toggle
                assert not find('Log and input$')
                toolbar(317)  # Settings is a fourth native viewport, not an embedded dialog
                settings_window = find('Settings$')[0]
                harness.close_window(display, settings_window)
                time.sleep(.3)
                assert not find('Settings$')
                config = json.loads((root / 'session/glulx-settings.json').read_text())
                assert not config['show_log_window'], 'Closing Settings did not save JSON'
                toolbar(185)
                inputs = find('Log and input$')[0]
                toolbar(245)  # Showing translation must not enable its network worker
                assert not find('Translation$')
                toolbar(245)
                translation = find('Translation$')[0]
                toolbar(317)
                settings_window = find('Settings$')[0]
                assert len({canvas, inputs, translation, settings_window}) == 4
                keys('windowraise', str(settings_window))
                subprocess.run(['import', '-window', str(settings_window), str(root / 'settings.png')], env=env, check=True, timeout=10)
                if args.font_dialogs:
                    keys('windowfocus', '--sync', str(settings_window))
                    keys('mousemove', '--window', str(settings_window), '75', '110', 'click', '1')
                    deadline = time.monotonic() + 5
                    while not find('^Choose font$') and time.monotonic() < deadline:
                        time.sleep(.1)
                    chooser = find('^Choose font$')[0]
                    keys('windowfocus', '--sync', str(chooser))
                    time.sleep(.5)
                    keys('key', 'alt+s')
                    deadline = time.monotonic() + 5
                    while find('^Choose font$') and time.monotonic() < deadline:
                        time.sleep(.1)
                    assert not find('^Choose font$'), 'System font chooser did not accept'
                    deadline = time.monotonic() + 5
                    while time.monotonic() < deadline:
                        config = json.loads((root / 'session/glulx-settings.json').read_text())
                        if config['system_font']: break
                        time.sleep(.1)
                    assert config['system_font'], config
                    keys('windowfocus', '--sync', str(settings_window))
                    keys('mousemove', '--window', str(settings_window), '205', '110', 'click', '1')
                    deadline = time.monotonic() + 5
                    while not find('^Choose font file') and time.monotonic() < deadline:
                        time.sleep(.1)
                    chooser = find('^Choose font file')[0]
                    keys('windowfocus', '--sync', str(chooser))
                    keys('key', 'Escape')
                    time.sleep(.3)
                    assert not find('^Choose font file'), 'File picker did not cancel'
                keys('windowsize', str(settings_window), '500', '500')
                keys('windowfocus', '--sync', str(inputs))
                keys('type', '--clearmodifiers', 'look')
                keys('key', 'Return')
                time.sleep(.4)
                harness.close_window(display, settings_window)
                time.sleep(.2)
                keys('windowsize', str(inputs), '420', '320')
                keys('windowsize', str(translation), '620', '400')
                time.sleep(.3)
                for window, title, shortcut in [(inputs, 'Log and input$', 'ctrl+shift+l'),
                                                 (translation, 'Translation$', 'ctrl+shift+t')]:
                    harness.close_window(display, window)
                    time.sleep(.3)
                    assert not find(title), f'{title} did not close'
                    assert find('^Glulx Player$') == [canvas], 'Closing a companion closed the app'
                    keys('windowfocus', '--sync', str(canvas))
                    keys('key', shortcut)
                    deadline = time.monotonic() + 3
                    while not find(title) and time.monotonic() < deadline:
                        time.sleep(.1)
                    assert find(title), f'{title} did not reopen'
                assert keys('getwindowgeometry', '--shell', str(canvas)).stdout == geometry
                diagnostics = (root / 'session/diagnostic.log').read_text()
                assert len(re.findall(r'\] resize ', diagnostics)) <= (3 if args.font_dialogs else 1), diagnostics

            session = harness.run_story(args.candidate.resolve(), story, root / 'session', display,
                                        exercise, focus_input=True, diagnostics=True)
            assert 'Received: look' in session['transcript'], session['transcript']
            config = json.loads((root / 'session/glulx-settings.json').read_text())
            assert config['show_log_window'] and config['show_translation_window']
            assert not config['translation']['enabled']
            assert '"glulx-rs-settings"' not in (root / 'session/data/glulxplayer/app.ron').read_text()
            print('PASS toolbar toggles; native Settings; portable JSON; independent input; resize isolation; close/reopen', flush=True)
        finally:
            server.terminate()
            server.wait(timeout=10)


if __name__ == '__main__':
    main()
