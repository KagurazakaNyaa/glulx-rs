#!/usr/bin/env python3
"""Linux GUI acceptance for external resources, selection and session restore.

Uses original generated media, a dynamically allocated Xvfb display and fresh
XDG directories. Requires xdotool, ImageMagick import/convert, and libX11.
Screenshots, transcripts and session files are retained under --output.
"""

import argparse
import importlib.util
import os
from pathlib import Path
import select
import shutil
import struct
import subprocess
import tempfile
import time
import zlib


def module(name):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(name + '.py'))
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


def fixture(root):
    reference = module('check-reference')
    maps = module('check-resource-maps')
    b = reference.StoryBuilder()
    mem = lambda address: (7, address)
    window, graphics, stream, value = [mem(address) for address in (0x800, 0x804, 0x808, 0x80c)]
    event = 0x810
    b.instruction(0x149, 2, 0)
    b.glk(0x23, [0, 0, 0, 3, 0], window)
    b.glk(0x23, [window, 0x12, 140, 5, 0], graphics)
    b.glk(0x2f, [window])
    b.text('Resource selection test; every key reloads Data and the picture.\n')
    b.labels['refresh'] = len(b.code)
    b.text('DATA:')
    b.glk(0x49, [1, 0], stream)
    b.instruction(0x22, stream, 'missing')
    b.glk(0x90, [stream], value)
    b.instruction(0x70, value)
    b.glk(0x44, [stream, 0])
    b.instruction(0x20, 'draw')
    b.labels['missing'] = len(b.code)
    b.text('-')
    b.labels['draw'] = len(b.code)
    b.glk(0xeb, [graphics, 0xffffff])
    b.glk(0x2a, [graphics])
    b.glk(0xe2, [graphics, 1, 20, 20, 160, 80], value)
    b.text(' DRAW:')
    b.instruction(0x71, value)
    b.instruction(0x70, 10)
    b.glk(0xd2, [window])
    b.labels['wait'] = len(b.code)
    b.glk(0xc0, [event])
    b.instruction(0x25, mem(event), 2, 'wait')
    b.text('KEY:')
    b.instruction(0x71, mem(event + 8))
    b.instruction(0x70, 10)
    b.instruction(0x20, 'refresh')
    image = bytes(b.finish())

    def chunk(tag, data):
        return tag + reference.u32(len(data)) + data + bytes(len(data) % 2)

    def png(color):
        def png_chunk(tag, data):
            return reference.u32(len(data)) + tag + data + reference.u32(zlib.crc32(tag + data))
        return (b'\x89PNG\r\n\x1a\n'
                + png_chunk(b'IHDR', struct.pack('>IIBBBBB', 16, 16, 8, 2, 0, 0, 0))
                + png_chunk(b'IDAT', zlib.compress((b'\0' + bytes(color) * 16) * 16))
                + png_chunk(b'IEND', b''))

    for label, color in [('A', (20, 40, 80)), ('B', (170, 35, 55))]:
        chunks = [(b'GLUL', b'Exec', 0, image),
                  (b'PNG ', b'Pict', 1, png(color)),
                  (b'TEXT', b'Data', 1, label.encode())]
        index = bytearray(reference.u32(len(chunks)))
        offset = 12 + 12 + 12 * len(chunks)
        for tag, usage, number, data in chunks:
            index.extend(usage + reference.u32(number) + reference.u32(offset))
            offset += 8 + len(data) + len(data) % 2
        body = b'IFRS' + chunk(b'RIdx', index)
        body += b''.join(chunk(tag, data) for tag, _, _, data in chunks)
        body += chunk(b'IFhd', image[:128])
        body += chunk(b'IFmd', f'<ifindex><title>Resource {label}</title></ifindex>'.encode())
        source = b'FORM' + reference.u32(len(body)) + body
        maps.split_archive(source, root / label)
    raw = root / 'game.ulx'
    raw.write_bytes(image)
    (root / 'new-story.ulx').write_bytes(image)
    bad = bytearray((root / 'A/story.blorb').read_bytes())
    bad[bad.index(b'IFhd') + 8 + 127] ^= 1
    (root / 'bad.blorb').write_bytes(bad)
    return raw


class Desktop:
    def __init__(self, candidate, directory, display, arguments, phase):
        self.directory, self.display, self.phase = directory, display, phase
        directory.mkdir(exist_ok=True)
        self.env = dict(os.environ, DISPLAY=display, LIBGL_ALWAYS_SOFTWARE='1',
                        XDG_CONFIG_HOME=str(directory / 'config'),
                        XDG_DATA_HOME=str(directory / 'data'),
                        XDG_CACHE_HOME=str(directory / 'cache'))
        self.log = (directory / f'{phase}.log').open('w')
        self.process = subprocess.Popen([str(candidate), *map(str, arguments)], env=self.env,
                                        stdout=self.log, stderr=self.log)
        self.window = None

    def keys(self, *arguments):
        subprocess.run(['xdotool', *map(str, arguments)], env=self.env, check=True, timeout=10)

    def click(self, x, y):
        self.keys('mousemove', x, y, 'click', 1)
        time.sleep(.35)

    def type_path(self, x, y, path):
        self.click(x, y)
        self.keys('key', 'ctrl+a')
        self.keys('type', '--clearmodifiers', str(path))
        time.sleep(.2)

    def choose_resources(self, choice, path=None, browse=False, directory=False):
        # Fresh XDG settings and a fixed 1100x760 viewport keep egui controls
        # at these positions. This exercises the same visible UI as a player.
        self.click(18, 12)
        self.click(100, 58)
        self.click(30, [127, 147, 168][choice])
        if choice == 2:
            if browse:
                self.click(340, 190)
                self.type_path(180, 319, path)
                self.keys('key', 'Return')
                time.sleep(.35)
                if directory:
                    self.click(80, 340)
            else:
                self.type_path(170, 190, path)
        self.screenshot('selection')
        self.click(100, 237 if choice == 2 else 216)
        time.sleep(.7)

    def open_story(self, path):
        self.click(18, 12)
        self.click(70, 37)
        self.type_path(180, 110, path)
        self.keys('key', 'Return')
        time.sleep(.7)

    def screenshot(self, name):
        path = self.directory / f'{self.phase}-{name}.png'
        subprocess.run(['import', '-window', 'root', str(path)], env=self.env, check=True, timeout=10)
        return path

    def color_pixels(self, name, rgb, tolerance=0):
        path = self.screenshot(name)
        tool = 'magick' if shutil.which('magick') else 'convert'
        data = subprocess.check_output([tool, str(path), '-depth', '8', 'rgb:-'], timeout=10)
        if tolerance == 0:
            return sum(data[i:i + 3] == bytes(rgb) for i in range(0, len(data), 3))
        return sum(all(abs(data[i + channel] - rgb[channel]) <= tolerance for channel in range(3))
                   for i in range(0, len(data), 3))

    def wait(self):
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            assert self.process.poll() is None, (self.directory / f'{self.phase}.log').read_text()
            result = subprocess.run(['xdotool', 'search', '--onlyvisible', '--name', 'Glulx Player'],
                                    env=self.env, capture_output=True, timeout=5)
            if result.returncode == 0 and result.stdout.strip():
                self.window = int(result.stdout.splitlines()[0])
                break
            time.sleep(.1)
        if self.window is None:
            self.screenshot('startup-timeout')
            subprocess.run(['xdotool', 'search', '--name', '.*', 'getwindowname', '%@'],
                           env=self.env, stdout=self.log, stderr=self.log, timeout=5)
            raise AssertionError(f'Desktop window did not appear; see {self.directory}')
        self.keys('windowfocus', '--sync', self.window)
        self.keys('windowmove', self.window, 0, 0)
        self.keys('windowsize', self.window, 1100, 760)
        time.sleep(2)

    def close(self):
        harness = module('check-input-ui')
        self.screenshot('final')
        harness.close_window(self.display, self.window)
        assert self.process.wait(timeout=15) == 0, (self.directory / f'{self.phase}.log').read_text()
        return harness.load_session(self.directory)

    def cleanup(self):
        if self.process.poll() is None:
            self.process.kill()
            self.process.wait()
        self.log.close()


def run_app(candidate, directory, display, arguments, exercise, phase='app'):
    desktop = Desktop(candidate, directory, display, arguments, phase)
    try:
        desktop.wait()
        exercise(desktop)
        return desktop.close()
    except BaseException:
        if desktop.window and desktop.process.poll() is None:
            desktop.screenshot('failure')
        raise
    finally:
        desktop.cleanup()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--candidate', type=Path, default=Path('target/debug/glulx-rs'))
    parser.add_argument('--output', type=Path)
    arguments = parser.parse_args()
    for executable in ['Xvfb', 'xdotool', 'import']:
        assert shutil.which(executable), f'Required executable missing: {executable}'
    if arguments.output:
        arguments.output.mkdir(parents=True, exist_ok=True)
    root = Path(tempfile.mkdtemp(prefix='glulx-resources-', dir=arguments.output)).resolve()
    print(f'Artifacts: {root}', flush=True)
    raw = fixture(root)
    read_fd, write_fd = os.pipe()
    with (root / 'xvfb.log').open('w') as log:
        server = subprocess.Popen(['Xvfb', '-displayfd', str(write_fd), '-noreset', '-screen', '0', '1280x900x24'],
                                  pass_fds=(write_fd,), stdout=log, stderr=log)
        os.close(write_fd)
        try:
            assert select.select([read_fd], [], [], 10)[0], 'Xvfb did not allocate a display'
            display = ':' + os.read(read_fd, 100).decode().strip()
            os.close(read_fd)
            assert display[1:].isdigit(), f'Xvfb failed to create a display; see {root / "xvfb.log"}'
            candidate = arguments.candidate.resolve()
            blue, red = (20, 40, 80), (170, 35, 55)
            initial = 'Resource selection test; every key reloads Data and the picture.\n'

            def check_key(desktop, key, color, phase):
                desktop.keys('key', key)
                time.sleep(.5)
                assert desktop.color_pixels(phase, color) >= 12000, f'Expected resource pixels after {key}'

            # CLI explicit loading and self-contained restoration. The second
            # key requests resources again, rather than only showing saved pixels.
            cli_raw, cli_archive = root / 'cli-game.ulx', root / 'cli-assets.blorb'
            shutil.copyfile(raw, cli_raw)
            shutil.copyfile(root / 'A/story.blorb', cli_archive)
            saved = run_app(candidate, root / 'cli', display,
                            [cli_raw, '--resources', cli_archive],
                            lambda app: check_key(app, 'k', blue, 'explicit'))
            assert saved['transcript'] == initial + 'DATA:A DRAW:1\nKEY:107\nDATA:A DRAW:1\n', saved
            cli_raw.unlink()
            cli_archive.unlink()
            restored = run_app(candidate, root / 'cli', display, [],
                               lambda app: check_key(app, 'r', blue, 'restored'), 'resume')
            assert restored['transcript'] == saved['transcript'] + 'KEY:114\nDATA:A DRAW:1\n', restored
            print('PASS explicit CLI resources and fresh resource reads after archive/story deletion and session restore', flush=True)

            # Exercise all three radio choices and both browser actions.
            shutil.copyfile(root / 'A/story.blorb', root / 'game.blorb')
            def select_resources(app):
                assert app.color_pixels('initial-none', blue) == 0
                app.choose_resources(0)
                assert app.color_pixels('automatic', blue) >= 12000
                app.choose_resources(1)
                assert app.color_pixels('embedded-only', blue) == 0
                app.choose_resources(2, root / 'A/story.blorb', browse=True)
                assert app.color_pixels('archive', blue) >= 12000
                app.choose_resources(2, root / 'B/loose', browse=True, directory=True)
                assert app.color_pixels('directory', red) >= 12000
                assert app.color_pixels('old-cache-cleared', blue) == 0
                check_key(app, 'b', red, 'directory-read')
            selected = run_app(candidate, root / 'gui', display,
                               [raw, '--no-auto-resources'], select_resources)
            assert selected['transcript'] == initial + 'DATA:B DRAW:1\nKEY:98\nDATA:B DRAW:1\n', selected
            shutil.rmtree(root / 'B/loose')
            resumed = run_app(candidate, root / 'gui', display, [],
                              lambda app: check_key(app, 'r', red, 'directory-restored'), 'resume')
            assert resumed['transcript'] == selected['transcript'] + 'KEY:114\nDATA:B DRAW:1\n', resumed
            print('PASS GUI automatic/embedded/explicit choices, archive Browse, directory selection, cache clearing and directory restoration', flush=True)

            # A rejected archive must retain the old VM, including its previous
            # key count; typing in the dialog must not become story characters.
            def reject_identity(app):
                check_key(app, 'k', blue, 'before-error')
                app.choose_resources(2, root / 'bad.blorb')
                assert app.color_pixels('identity-error', (170, 45, 40), tolerance=8) > 100, 'Missing visible resource error'
            rejected = run_app(candidate, root / 'invalid', display,
                               [raw, '--resources', root / 'A/story.blorb'], reject_identity)
            assert rejected['transcript'] == initial + 'DATA:A DRAW:1\nKEY:107\nDATA:A DRAW:1\n', rejected
            recovered = run_app(candidate, root / 'invalid', display, [],
                                lambda app: check_key(app, 'r', blue, 'retained'), 'resume')
            assert recovered['transcript'] == rejected['transcript'] + 'KEY:114\nDATA:A DRAW:1\n', recovered
            print('PASS mismatched IFhd preserves old state and resource-dialog typing does not reach the game', flush=True)

            def next_story(app):
                assert app.color_pixels('old-story', blue) >= 12000
                app.open_story(root / 'new-story.ulx')
                assert app.color_pixels('new-story', blue) == 0, 'External resources leaked into another story'
                app.keys('key', 'n')
                time.sleep(.4)
            new = run_app(candidate, root / 'new', display,
                          [raw, '--resources', root / 'A/story.blorb'], next_story)
            assert new['transcript'] == initial + 'DATA:- DRAW:0\nKEY:110\nDATA:- DRAW:0\n', new
            print('PASS opening another story does not retain the old external resource selection', flush=True)
        finally:
            server.terminate()
            server.wait(timeout=10)


if __name__ == '__main__':
    main()
