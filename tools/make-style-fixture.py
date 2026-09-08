#!/usr/bin/env python3
"""Generate an original story for paragraph styles, grid links and grid editing."""
import importlib.util
import pathlib
import struct
import sys

spec = importlib.util.spec_from_file_location('reference', pathlib.Path(__file__).with_name('check-reference.py'))
reference = importlib.util.module_from_spec(spec)
spec.loader.exec_module(reference)
u32 = reference.u32


def story():
    b = reference.StoryBuilder()
    mem = lambda address: (7, address)
    strings = bytearray()
    def text(value):
        address = 0x2200 + len(strings)
        strings.extend(b'\xe0' + value.encode('ascii') + b'\0')
        b.glk(0x82, [address])
    buffer, grid = mem(0x2000), mem(0x2004)
    b.instruction(0x149, 2, 0)
    for kind, style, hint, value in [(3, 3, 2, 2), (3, 9, 0, 3), (3, 9, 1, -2), (3, 9, 2, 1),
                                     (3, 10, 2, 3), (4, 9, 7, 0xffffff), (4, 9, 8, 0x204090)]:
        b.glk(0xb0, [kind, style, hint, value])
    b.glk(0x23, [0, 0, 0, 3, 0], buffer)
    b.glk(0x23, [buffer, 0x12, 5, 4, 0], grid)
    b.glk(0x2f, [buffer])
    b.glk(0x86, [3])
    text('Centered heading\n')
    b.glk(0x86, [9])
    text(('This paragraph has a hanging first line, side indentation and full justification. '
          'Words should reach both edges on wrapped lines while the last line remains short. ')*4 + '\n')
    b.glk(0x86, [10])
    text('Right-aligned ending\n')
    b.glk(0x86, [0])
    text('Click LINK in the grid, then edit the prefilled grid input and press Enter.\n')
    b.glk(0x2f, [grid])
    b.glk(0x86, [9])
    b.glk(0x100, [77])
    text('LINK')
    b.glk(0x100, [0])
    b.glk(0x86, [0])
    text('  Styled grid\n')
    b.glk(0x2b, [grid, 0, 2])
    text('Name: ')
    b.glk(0x102, [grid])
    b.labels['wait'] = len(b.code)
    b.glk(0xc0, [0x2010])
    b.instruction(0x24, mem(0x2010), 8, 'link')
    b.instruction(0x25, mem(0x2010), 3, 'wait')
    b.glk(0x2f, [buffer])
    text('Grid line input completed.\n')
    b.instruction(0x20, 'wait')
    b.labels['link'] = len(b.code)
    b.glk(0x2f, [buffer])
    text('Grid hyperlink received.\n')
    b.instruction(0x40, 0x41646100, mem(0x2100))  # initial Latin1 "Ada"
    b.glk(0xd0, [grid, 0x2100, 50, 3])
    b.instruction(0x20, 'wait')
    image = bytearray(0x4000)
    image[:4] = b'Glul'
    for offset, value in [(4, 0x30103), (8, 0x2000), (12, 0x4000), (16, 0x4000), (20, 0x1000), (24, 0x40)]:
        image[offset:offset+4] = u32(value)
    for offset, label in b.fixups:
        b.code[offset:offset+4] = u32(b.labels[label] - (offset + 4) + 2)
    assert len(b.code) + 0x40 <= 0x2000
    assert len(strings) + 0x2200 <= len(image)
    image[0x40:0x40+len(b.code)] = b.code
    image[0x2200:0x2200+len(strings)] = strings
    image[32:36] = u32(sum(struct.unpack('>4096I', image)))
    return image


if __name__ == '__main__':
    output = pathlib.Path(sys.argv[1])
    output.write_bytes(story())
    print(output)
