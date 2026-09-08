#!/usr/bin/env python3
"""Create an original, redistributable Glk image and MOD audio test story.

Usage: python3 tools/make-media-fixture.py /tmp/glulx-media.gblorb
Uses only the Python standard library. No downloaded game/media is embedded.
"""
import importlib.util
import pathlib
import struct
import sys
import zlib

spec = importlib.util.spec_from_file_location('reference', pathlib.Path(__file__).with_name('check-reference.py'))
reference = importlib.util.module_from_spec(spec)
spec.loader.exec_module(reference)
u32 = reference.u32


def png(width, height):
    def chunk(kind, data):
        return u32(len(data)) + kind + data + u32(zlib.crc32(kind + data))
    rows = bytearray()
    for y in range(height):
        rows.append(0)
        for x in range(width):
            border = x < 3 or y < 3 or x >= width - 3 or y >= height - 3
            rows.extend((20, 40, 80, 255) if border else (40 + x * 120 // width, 110, 230 - y * 120 // height, 255))
    return (b'\x89PNG\r\n\x1a\n'
            + chunk(b'IHDR', struct.pack('>IIBBBBB', width, height, 8, 6, 0, 0, 0))
            + chunk(b'IDAT', zlib.compress(bytes(rows))) + chunk(b'IEND', b''))


def mod_music():
    song = bytearray(1084 + 1024 + 64)
    song[:14] = b'Glulx MOD test'
    song[42:44] = struct.pack('>H', 32)
    song[45] = 64
    song[48:50] = struct.pack('>H', 32)
    song[950] = 1
    song[1080:1084] = b'M.K.'
    song[2108:2140] = bytes([96])*32
    song[2140:2172] = bytes([160])*32
    song[1084:1088] = bytes([0x01, 0xac, 0x1f, 0x03])
    song[1100:1104] = bytes([0, 0, 0x0c, 0])
    song[1116:1120] = bytes([0, 0, 0x0b, 0])
    return song


def story():
    b = reference.StoryBuilder()
    mem = lambda address: (7, address)
    strings = bytearray()
    def text(value):
        address = 0x2200 + len(strings)
        strings.extend(b'\xe0' + value.encode('ascii') + b'\0')
        b.glk(0x82, [address])
    window = mem(0x2000)
    b.instruction(0x149, 2, 0)
    b.glk(0x23, [0, 0, 0, 3, 0], window)
    b.glk(0x2f, [window])
    b.glk(0x86, [3])
    text('Glk image layout\n')
    b.glk(0x86, [0])
    text('Inline: baseline ')
    b.glk(0xe2, [window, 1, 1, 0, 60, 70])
    text(' top ')
    b.glk(0xe2, [window, 1, 2, 0, 60, 70])
    text(' center ')
    b.glk(0xe2, [window, 1, 3, 0, 60, 70])
    text(' end.\n\n')
    b.glk(0xec, [window, 1, 4, 0, 16384, 65536, 3 | 12, 65536])
    b.glk(0xec, [window, 1, 5, 0, 16384, 65536, 3 | 12, 65536])
    text(('This paragraph flows between two margin pictures. Resize the window to see '
          'the pictures and words reflow together. ')*8)
    b.glk(0xe8, [window])
    text('Below both margins after a flow break.\n\n')
    b.glk(0x100, [77])
    b.glk(0xe2, [window, 1, 1, 0, 150, 75])
    b.glk(0x100, [0])
    text(' Click this picture, or press Enter.\n')
    b.glk(0xf2, [0], mem(0x2004))
    b.glk(0xf9, [mem(0x2004), 2, 1, 99], mem(0x2008))
    b.glk(0x102, [window])
    b.glk(0xd0, [window, 0x2100, 100, 0])
    b.labels['wait'] = len(b.code)
    b.glk(0xc0, [0x2010])
    b.instruction(0x24, mem(0x2010), 7, 'audio')
    b.instruction(0x24, mem(0x2010), 3, 'line')
    b.instruction(0x25, mem(0x2010), 8, 'wait')
    text('Image hyperlink received.\n')
    b.glk(0x102, [window])
    b.instruction(0x20, 'wait')
    b.labels['line'] = len(b.code)
    text('Line input received.\n')
    b.glk(0xd0, [window, 0x2100, 100, 0])
    b.instruction(0x20, 'wait')
    b.labels['audio'] = len(b.code)
    text('MOD playback completed.\n')
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
    picture = png(320, 160)
    picture_offset = 12 + 36 + 8 + len(image)
    sound_offset = picture_offset + 8 + len(picture) + len(picture) % 2
    index = (u32(2) + b'Pict' + u32(1) + u32(picture_offset)
             + b'Snd ' + u32(2) + u32(sound_offset))
    chunks = [(b'RIdx', index), (b'GLUL', image),
              (b'PNG ', picture), (b'MOD ', mod_music())]
    body = b'IFRS' + b''.join(kind + u32(len(data)) + data + b'\0' * (len(data) % 2) for kind, data in chunks)
    return b'FORM' + u32(len(body)) + body


if __name__ == '__main__':
    output = pathlib.Path(sys.argv[1])
    output.write_bytes(story())
    print(output)
