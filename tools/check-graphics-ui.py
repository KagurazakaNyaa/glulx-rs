#!/usr/bin/env python3
"""Linux desktop regression for canvas resize, huge scaled images and restore.

Uses original generated content and the Xvfb prerequisites of check-input-ui.py.
"""
import argparse
import importlib.util
import os
from pathlib import Path
import select
import shutil
import subprocess
import tempfile
import time


def module(name):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(name + '.py'))
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


def story():
    ref = module('check-reference')
    media = module('make-media-fixture')
    b = ref.StoryBuilder()
    mem = lambda addr: (7, addr)
    buffer, graphics = mem(0x800), mem(0x804)
    b.instruction(0x149, 2, 0)
    b.glk(0x23, [0, 0, 0, 3, 0], buffer)
    b.glk(0x23, [buffer, 0x12, 250, 5, 0], graphics)
    b.glk(0x2f, [buffer])
    b.text('Resize, then press H for huge image.\n')
    b.glk(0xeb, [graphics, 0x112233])
    b.glk(0x2a, [graphics])
    b.glk(0xea, [graphics, 0xff0000, 5, 5, 20, 20])
    b.glk(0xea, [graphics, 0x00ff00, 500, 5, 50, 20])
    b.labels['request'] = len(b.code)
    b.glk(0xd2, [buffer])
    b.labels['wait'] = len(b.code)
    b.glk(0xc0, [0x810])
    b.instruction(0x25, mem(0x810), 2, 'wait')
    b.glk(0xe2, [graphics, 1, -0x80000000, -1, 0xffffffff, 0xffffffff], mem(0x808))
    b.text('HUGE:')
    b.instruction(0x71, mem(0x808))
    b.instruction(0x70, 10)
    b.instruction(0x20, 'request')
    image = bytes(b.finish())
    picture = media.png(4, 2)  # A solid original dark blue image.
    index = ref.u32(2) + b'Exec' + ref.u32(0) + ref.u32(48)
    index += b'Pict' + ref.u32(1) + ref.u32(48 + 8 + len(image))
    result = bytearray(b'FORM\0\0\0\0IFRS')
    for kind, data in [(b'RIdx', index), (b'GLUL', image), (b'PNG ', picture)]:
        result += kind + ref.u32(len(data)) + data + b'\0' * (len(data) % 2)
    result[4:8] = ref.u32(len(result) - 8)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--candidate', type=Path, default=Path('target/debug/glulx-rs'))
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    if args.output:
        args.output.mkdir(parents=True, exist_ok=True)
    root = Path(tempfile.mkdtemp(prefix='glulx-graphics-', dir=args.output)).resolve()
    print(f'Artifacts: {root}', flush=True)
    fixture = root / 'graphics.gblorb'
    fixture.write_bytes(story())
    harness = module('check-input-ui')
    read_fd, write_fd = os.pipe()
    with (root / 'xvfb.log').open('w') as log:
        xserver = subprocess.Popen(['Xvfb', '-noreset', '-displayfd', str(write_fd), '-screen', '0', '1280x900x24'],
                                   pass_fds=(write_fd,), stdout=log, stderr=log)
        os.close(write_fd)
        try:
            assert select.select([read_fd], [], [], 10)[0]
            display = ':' + os.read(read_fd, 100).decode().strip()
            os.close(read_fd)
            env = dict(os.environ, DISPLAY=display)

            def pixels(name):
                path = root / (name + '.png')
                subprocess.run(['import', '-window', 'root', str(path)], env=env, check=True, timeout=10)
                data = subprocess.check_output(['magick' if shutil.which('magick') else 'convert', str(path), '-depth', '8', 'rgb:-'], timeout=10)
                counts = {}
                for index in range(0, len(data), 3):
                    rgb = data[index:index+3]
                    counts[rgb] = counts.get(rgb, 0) + 1
                return counts

            def exercise(keys):
                before = pixels('before')
                assert before.get(b'\xff\0\0', 0) >= 350, before.get(b'\xff\0\0', 0)
                assert before.get(b'\0\xff\0', 0) >= 900, before.get(b'\0\xff\0', 0)
                keys('getwindowfocus', 'windowsize', '500', '820')
                time.sleep(.7)
                keys('getwindowfocus', 'windowsize', '1100', '820')
                time.sleep(.7)
                after = pixels('regrown')
                assert after.get(b'\0\xff\0', 0) == 0, 'Cropped green pixels reappeared'
                assert after.get(b'\xff\0\0', 0) >= 350, 'Visible red pixels lost'
                assert after.get(b'\x11\x22\x33', 0) > 200000, 'New area not filled with background'
                keys('key', 'h')
                time.sleep(1)
                assert pixels('huge').get(b'\x14\x28\x50', 0) > 200000

            saved = harness.run_story(args.candidate.resolve(), fixture, root / 'session', display, exercise)
            assert 'HUGE:1' in saved['transcript'], saved['transcript']
            def resume(_keys):
                assert pixels('restored').get(b'\x14\x28\x50', 0) > 200000
            restored = harness.run_story(args.candidate.resolve(), None, root / 'session', display, resume, 'resume')
            assert restored['transcript'] == saved['transcript']
            print('PASS canvas crop/regrow, current background, huge unsigned image clipping and session restore')
        finally:
            xserver.terminate()
            xserver.wait(timeout=10)


if __name__ == '__main__':
    main()
