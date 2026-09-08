#!/usr/bin/env python3
"""Generate an original SONG/AIFF story for playback and session checks.

Usage: python3 tools/make-song-fixture.py /tmp/glulx-song.gblorb
The generated music lasts 7.68 seconds per repetition and starts with two
repetitions, leaving time to close/reopen the player and verify resumption.
No external audio files or encoders are needed.
"""
import importlib.util
import pathlib
import struct
import sys


spec = importlib.util.spec_from_file_location(
    'reference', pathlib.Path(__file__).with_name('check-reference.py'))
reference = importlib.util.module_from_spec(spec)
spec.loader.exec_module(reference)
u32 = reference.u32


def chunk(kind, data):
    return kind + u32(len(data)) + data + b'\0' * (len(data) % 2)


def song(volume=64):
    data = bytearray(1084 + 1024)
    data[:15] = b'Glulx SONG test'
    for index, (tune, loudness) in enumerate([(0, volume), (1, volume // 2)]):
        start = 20 + index * 30
        data[start:start + 5] = b'SND42'
        data[start + 22:start + 30] = bytes([255, 255, tune, loudness, 255, 255, 255, 255])
    data[950] = 1
    data[1080:1084] = b'M.K.'
    data[1084:1088] = bytes([0x01, 0xac, 0x1f, 0x06])
    data[1088:1092] = bytes([0x01, 0x53, 0x20, 0])
    data[1084 + 63 * 16:1088 + 63 * 16] = bytes([0, 0, 0x0b, 0])
    return data


def aiff():
    # A 16-bit stereo square wave, averaged to mono by the SONG assembler.
    common = struct.pack('>HIH', 2, 64, 16) + bytes.fromhex('400eac44000000000000')
    pcm = b''.join(struct.pack('>hh', 24576 if i < 32 else -24576,
                              12288 if i < 32 else -12288) for i in range(64))
    sound = u32(7) + u32(0) + b'OFFSET!' + pcm
    markers = struct.pack('>H', 2)
    for marker, position, name in [(300, 4, b'begin'), (701, 60, b'end!')]:
        label = bytes([len(name)]) + name
        markers += struct.pack('>HI', marker, position) + label + b'\0' * (len(label) % 2)
    instrument = bytes([60, 0, 0, 127, 1, 127, 0, 0]) + struct.pack('>6H', 2, 300, 701, 0, 0, 0)
    body = b'AIFF' + chunk(b'SSND', sound) + chunk(b'MARK', markers)
    body += chunk(b'COMM', common) + chunk(b'INST', instrument)
    return b'FORM' + u32(len(body)) + body


def executable():
    b = reference.StoryBuilder()
    mem = lambda address: (7, address)
    strings = bytearray()

    def text(value):
        address = 0x2400 + len(strings)
        strings.extend(b'\xe0' + value.encode('ascii') + b'\0')
        b.glk(0x82, [address])

    window, channel, second = mem(0x2000), mem(0x2004), mem(0x2008)
    event, command, result = 0x2010, mem(0x2020), mem(0x2024)
    b.instruction(0x149, 2, 0)
    b.glk(0x23, [0, 0, 0, 3, 0], window)
    b.glk(0x2f, [window])
    text('SONG with shared AIFF instruments\n'
         'Original generated notes; sustain ping-pong, 16-bit stereo and SSND offset.\n'
         'Commands: p pause, r resume, s stop, n play twice, m play two channels, '
         'v fade volume, q quit.\n'
         'Close and reopen the player while music is active to check resumption.\n')
    b.glk(0xf2, [11], channel)
    b.glk(0xf2, [12], second)
    b.labels['play'] = len(b.code)
    b.glk(0xfb, [channel, 65536])
    b.glk(0xff, [channel])
    b.glk(0xf9, [channel, 7, 2, 99], result)
    b.instruction(0x23, result, 'started')
    text('SONG playback failed or no audio device is available.\n')
    b.instruction(0x20, 'input')
    b.labels['started'] = len(b.code)
    text('SONG playback started (2 repeats).\n')
    b.labels['input'] = len(b.code)
    b.glk(0xd0, [window, 0x2100, 32, 0])
    b.labels['wait'] = len(b.code)
    b.glk(0xc0, [event])
    b.instruction(0x24, mem(event), 7, 'finished')
    b.instruction(0x24, mem(event), 9, 'faded')
    b.instruction(0x25, mem(event), 3, 'wait')
    b.instruction(0x4a, 0x2100, 0, command)
    for key, label in [('p', 'pause'), ('r', 'resume'), ('s', 'stop'),
                       ('n', 'play'), ('m', 'multi'), ('v', 'fade'), ('q', 'quit')]:
        b.instruction(0x24, command, ord(key), label)
    b.instruction(0x20, 'input')
    for label, selector, message in [('pause', 0xfe, 'SONG paused.\n'),
                                     ('resume', 0xff, 'SONG resumed.\n'),
                                     ('stop', 0xfa, 'SONG stopped.\n')]:
        b.labels[label] = len(b.code)
        b.glk(selector, [channel])
        text(message)
        b.instruction(0x20, 'input')
    b.labels['multi'] = len(b.code)
    b.glk(0xff, [channel])
    b.instruction(0x40, channel, mem(0x2040))
    b.instruction(0x40, second, mem(0x2044))
    b.instruction(0x40, 7, mem(0x2048))
    b.instruction(0x40, 8, mem(0x204c))
    b.glk(0xf7, [0x2040, 2, 0x2048, 2, 77], result)
    text('SONG simultaneous channels started: ')
    b.instruction(0x71, result)
    text('\n')
    b.instruction(0x20, 'input')
    b.labels['fade'] = len(b.code)
    b.glk(0xfd, [channel, 16384, 1000, 100])
    text('SONG volume fading.\n')
    b.instruction(0x20, 'input')
    b.labels['finished'] = len(b.code)
    text('SONG playback completed.\n')
    b.instruction(0x20, 'wait')
    b.labels['faded'] = len(b.code)
    text('SONG fade completed.\n')
    b.instruction(0x20, 'wait')
    b.labels['quit'] = len(b.code)
    b.instruction(0x120)
    image = b.finish(ram_start=0x2000, ext_start=0x4000, end_mem=0x4000)
    assert 0x2400 + len(strings) <= len(image)
    image[0x2400:0x2400 + len(strings)] = strings
    image[32:36] = bytes(4)
    image[32:36] = u32(sum(word[0] for word in struct.iter_unpack('>I', image)))
    return image


def story():
    resources = [(b'Exec', 0, b'GLUL', executable()),
                 (b'Snd ', 7, b'SONG', song()),
                 (b'Snd ', 8, b'SONG', song(48)),
                 (b'Snd ', 42, b'FORM', aiff()[8:])]
    index = u32(len(resources))
    offset = 12 + 8 + 4 + 12 * len(resources)
    for usage, number, kind, data in resources:
        index += usage + u32(number) + u32(offset)
        offset += 8 + len(data) + len(data) % 2
    descriptions = u32(1) + b'Snd ' + u32(7)
    label = b'Two original square-wave notes from a shared AIFF instrument.'
    descriptions += u32(len(label)) + label
    body = b'IFRS' + chunk(b'RIdx', index)
    body += b''.join(chunk(kind, data) for _, _, kind, data in resources)
    body += chunk(b'RDes', descriptions)
    return b'FORM' + u32(len(body)) + body


if __name__ == '__main__':
    output = pathlib.Path(sys.argv[1])
    output.write_bytes(story())
    print(output)
