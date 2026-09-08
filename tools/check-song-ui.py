#!/usr/bin/env python3
"""Exercise original SONG audio, controls, notifications and desktop restore.

Uses the Linux Xvfb requirements of check-input-ui.py. No media is downloaded.
"""
import argparse
import importlib.util
import os
from pathlib import Path
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
    parser.add_argument('--candidate', type=Path, default=Path('target/debug/glulx-rs'))
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    if args.output:
        args.output.mkdir(parents=True, exist_ok=True)
    root = Path(tempfile.mkdtemp(prefix='glulx-song-', dir=args.output)).resolve()
    print(f'Artifacts: {root}', flush=True)
    story = root / 'song.gblorb'
    story.write_bytes(module('make-song-fixture').story())
    harness = module('check-input-ui')
    read_fd, write_fd = os.pipe()
    with (root / 'xvfb.log').open('w') as log:
        server = subprocess.Popen(['Xvfb', '-displayfd', str(write_fd), '-screen', '0', '1280x900x24'],
                                  pass_fds=(write_fd,), stdout=log, stderr=log)
        os.close(write_fd)
        try:
            assert select.select([read_fd], [], [], 10)[0]
            display = ':' + os.read(read_fd, 100).decode().strip()
            os.close(read_fd)

            def command(keys, value):
                keys('type', '--clearmodifiers', value)
                keys('key', 'Return')
                time.sleep(.3)

            def pause(keys):
                command(keys, 'p')
            saved = harness.run_story(args.candidate.resolve(), story, root / 'session', display, pause)
            assert 'SONG playback started (2 repeats).' in saved['transcript'], saved['transcript']
            assert 'SONG paused.' in saved['transcript']
            assert 'SONG playback completed.' not in saved['transcript']
            raw = (root / 'session/data/glulxplayer/app.ron').read_text()
            assert 'paused:true,resource:7' in raw and 'active:true' in raw
            print('PASS SONG playback, pause and active desktop snapshot', flush=True)

            def resume_and_finish(keys):
                command(keys, 'r')
                command(keys, 'v')
                time.sleep(15)
                command(keys, 'm')
                time.sleep(8.2)
                command(keys, 'n')
                command(keys, 's')
            restored = harness.run_story(args.candidate.resolve(), None, root / 'session', display,
                                         resume_and_finish, 'resume')
            output = restored['transcript']
            assert 'SONG resumed.' in output and 'SONG fade completed.' in output
            assert 'SONG simultaneous channels started: 2' in output, output
            assert output.count('SONG playback completed.') == 3, output
            assert 'SONG stopped.' in output and 'failed' not in output
            print('PASS SONG restored resume, fade notification, final-repeat notification, synchronized channels and stop')
        finally:
            server.terminate()
            server.wait(timeout=10)


if __name__ == '__main__':
    main()
