#!/usr/bin/env python3
"""Generate an original Glulx fixture for all advertised desktop special keys.

Usage: python3 tools/make-input-fixture.py /tmp/input-keys.ulx
The fixture filters Arrange events, prints each received keycode, then times
out a partially composed line and re-requests input prefilled with NEW.
It waits indefinitely so transcript and composition can be checked in a session.
"""
import argparse
import importlib.util
from pathlib import Path

KEYS = [
    ('Left', -2), ('Right', -3), ('Up', -4), ('Down', -5),
    ('Delete', -7), ('BackSpace', -7), ('Escape', -8), ('Tab', -9),
    ('Prior', -10), ('Next', -11), ('Home', -12), ('End', -13),
] + [(f'F{number}', -16 - number) for number in range(1, 13)] + [('Return', -6)]


def story():
    source = Path(__file__).with_name('check-reference.py')
    spec = importlib.util.spec_from_file_location('reference_fixture', source)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    builder = module.StoryBuilder()
    mem = lambda address: (7, address)
    builder.instruction(0x149, 2, 0)
    builder.glk(0x23, [0, 0, 0, 3, 0], mem(0x8000))
    builder.glk(0x2f, [mem(0x8000)])
    for key, _ in KEYS:
        builder.text(f'Press {key}:\n')
        builder.glk(0xd2, [mem(0x8000)])
        builder.labels[f'wait-{key}'] = len(builder.code)
        builder.glk(0xc0, [0x8010])
        builder.instruction(0x25, mem(0x8010), 2, f'wait-{key}')
        builder.text('KEY:')
        builder.instruction(0x71, mem(0x8018))
        builder.instruction(0x70, 10)
    builder.text('All character keys received.\n')
    builder.text('Type abc before the timer expires.\n')
    builder.glk(0xd0, [mem(0x8000), 0x8100, 40, 0])
    builder.glk(0xd6, [2000])
    builder.labels['timer'] = len(builder.code)
    builder.glk(0xc0, [0x8010])
    builder.instruction(0x25, mem(0x8010), 1, 'timer')
    builder.glk(0xd1, [mem(0x8000), 0x8010])
    builder.glk(0xd6, [0])
    builder.text('CANCELLED:')
    builder.instruction(0x71, mem(0x8018))
    builder.text(' COMPOSED:')
    builder.glk(0x84, [0x8100, mem(0x8018)])
    builder.instruction(0x70, 10)
    for index, character in enumerate('NEW'):
        builder.instruction(0x42, ord(character), mem(0x8100 + index))
    builder.glk(0xd0, [mem(0x8000), 0x8100, 40, 3])
    builder.labels['idle'] = len(builder.code)
    builder.glk(0xc0, [0x8010])
    builder.instruction(0x20, 'idle')
    return builder.finish(0x8000, 0x9000, 0x9100)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    args.output.write_bytes(story())
    print(f'Wrote {args.output}: {len(KEYS)} key events')
