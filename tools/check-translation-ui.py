#!/usr/bin/env python3
"""Check future-only translation toggles and cache/coalescing through native UI.

Uses an original fixture, an isolated loopback HTTP server and the Xvfb
requirements of check-input-ui.py. No external translation service is contacted.
"""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import select
import subprocess
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


def module(name):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(name + '.py'))
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--candidate', type=Path, default=Path('target/release/glulx-rs'))
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    if args.output:
        args.output.mkdir(parents=True, exist_ok=True)
    root = Path(tempfile.mkdtemp(prefix='glulx-translation-', dir=args.output)).resolve()
    print(f'Artifacts: {root}', flush=True)
    requests, arrived, release = [], threading.Event(), threading.Event()

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass

        def do_POST(self):
            requests.append(json.loads(self.rfile.read(int(self.headers['Content-Length']))))
            arrived.set()
            assert release.wait(15), 'Timed out waiting for UI coalescing exercise'
            body = json.dumps({'choices': [{'message': {'content': 'Translated paragraph.'}}]}).encode()
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    worker = threading.Thread(target=server.serve_forever, daemon=True)
    worker.start()
    directory = root / 'session'
    directory.mkdir()
    (directory / 'glulx-settings.json').write_text(json.dumps({'language': 'en', 'translation': {
        'enabled': False, 'endpoint': f'http://127.0.0.1:{server.server_port}/translate',
        'api_key': '', 'model': 'fixture', 'target_language': 'zh-CN',
    }}))
    harness = module('check-input-ui')
    b = module('check-reference').StoryBuilder()
    mem = lambda address: (7, address)
    b.instruction(0x149, 2, 0)
    b.glk(0x23, [0, 0, 0, 3, 0], mem(0x800))
    b.glk(0x2f, [mem(0x800)])
    b.text('Historical paragraph.\n')
    b.labels['request'] = len(b.code)
    b.glk(0xd0, [mem(0x800), 0x900, 32, 0])
    b.labels['wait'] = len(b.code)
    b.glk(0xc0, [0x810])
    b.instruction(0x25, mem(0x810), 3, 'wait')
    b.instruction(0x4a, 0x900, 0, mem(0x820))
    b.instruction(0x25, mem(0x820), ord('n'), 'append')
    b.glk(0x2a, [mem(0x800)])
    b.labels['append'] = len(b.code)
    b.text('Repeated paragraph.\n')
    b.instruction(0x20, 'request')
    story = root / 'translation.ulx'
    story.write_bytes(b.finish(0x800, 0x1000, 0x1100))
    read_fd, write_fd = os.pipe()
    with (root / 'xvfb.log').open('w') as log:
        display_server = subprocess.Popen(['Xvfb', '-noreset', '-displayfd', str(write_fd), '-screen', '0', '1280x900x24'], pass_fds=(write_fd,), stdout=log, stderr=log)
        os.close(write_fd)
        try:
            assert select.select([read_fd], [], [], 10)[0]
            display = ':' + os.read(read_fd, 100).decode().strip()
            os.close(read_fd)

            def exercise(keys):
                def window(title):
                    return int(keys('search', '--onlyvisible', '--name', title).stdout.splitlines()[0])
                inputs, translations = window('Log and input$'), window('Translation$')

                def toggle():
                    keys('windowraise', str(translations))
                    keys('windowfocus', '--sync', str(translations))
                    keys('mousemove', '--window', str(translations), '18', '16', 'click', '1')
                    time.sleep(.3)

                def command(value='repeat'):
                    keys('windowraise', str(inputs))
                    keys('windowfocus', '--sync', str(inputs))
                    keys('type', '--clearmodifiers', value)
                    keys('key', 'Return')
                    time.sleep(.3)

                toggle()
                assert not requests, 'Enabling submitted historical text'
                command()
                assert arrived.wait(3), 'New output was not submitted'
                command()  # Same text while the first request is still running.
                toggle()   # Existing requests should still complete while disabled.
                release.set()
                deadline = time.monotonic() + 5
                while time.monotonic() < deadline:
                    trace = (directory / 'diagnostic.log').read_text()
                    if trace.count('translation completed') == 2:
                        break
                    time.sleep(.1)
                assert trace.count('translation completed') == 2, trace
                command('next')  # Disabled output must remain untranslated, even if cached.
                toggle()
                command('next')  # Completed same-text cache must survive off/on.
                time.sleep(.4)
                trace = (directory / 'diagnostic.log').read_text()
                assert len(requests) == 1, requests
                assert requests[0]['messages'][-1]['content'] == 'Repeated paragraph.\n'
                assert trace.count('translation queued') == 2, trace
                assert trace.count('translation cached') == 1, trace
                assert 'translation cached turn=4' in trace, trace

            session = harness.run_story(args.candidate.resolve(), story, directory, display, exercise, focus_input=True, diagnostics=True)
            assert session['transcript'].count('Repeated paragraph.') == 4
            print('PASS no historical backfill; one HTTP request for duplicate content; both pending results retained; cache survives toggles', flush=True)
        finally:
            release.set()
            display_server.terminate()
            display_server.wait(timeout=10)
            server.shutdown()
            server.server_close()
            worker.join(timeout=5)


if __name__ == '__main__':
    main()
