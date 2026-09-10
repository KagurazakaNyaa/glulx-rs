#!/usr/bin/env python3
"""Reproducible Glulxe/Git differential and bidirectional IFZS smoke test.

Usage: python3 tools/check-reference.py --reference /path/to/glulxe --candidate target/debug/glulx-rs [--git /path/to/git]
Requires Python 3; generates a synthetic story in a temporary directory.
"""
import argparse
import pathlib
import re
import struct
import subprocess
import tempfile


def u32(value):
    return struct.pack('>I', value & 0xffffffff)


class StoryBuilder:
    def __init__(self):
        self.code = bytearray(b'\xc1\0\0')
        self.labels = {}
        self.fixups = []

    def instruction(self, opcode, *operands):
        self.code.extend(bytes([opcode]) if opcode < 128 else struct.pack('>H', opcode | 0x8000))
        modes = [operand[0] if isinstance(operand, tuple) else 3 for operand in operands]
        for index in range(0, len(modes), 2):
            self.code.append(modes[index] | ((modes[index + 1] if index + 1 < len(modes) else 0) << 4))
        for mode, operand in zip(modes, operands):
            value = operand[1] if isinstance(operand, tuple) else operand
            if mode in (0, 8):
                continue
            assert mode in (3, 7)
            if isinstance(value, str):
                self.fixups.append((len(self.code), value))
                self.code.extend(b'\0' * 4)
            else:
                self.code.extend(u32(value))

    def glk(self, selector, arguments, destination=(0, 0)):
        for argument in reversed(arguments):
            self.instruction(0x40, argument, (8, 0))
        self.instruction(0x130, selector, len(arguments), destination)

    def text(self, text):
        for character in text:
            self.instruction(0x70, ord(character))

    def finish(self, ram_start=0x800, ext_start=0x1000, end_mem=0x1100):
        image = bytearray(ext_start)
        image[:4] = b'Glul'
        for offset, value in [(4, 0x30103), (8, ram_start), (12, ext_start), (16, end_mem), (20, 0x1000), (24, 0x40)]:
            image[offset:offset+4] = u32(value)
        for offset, label in self.fixups:
            self.code[offset:offset+4] = u32(self.labels[label] - (offset + 4) + 2)
        assert len(self.code) < ram_start - 0x40
        image[0x40:0x40+len(self.code)] = self.code
        image[32:36] = u32(sum(word[0] for word in struct.iter_unpack('>I', image)))
        return image


def story():
    b = StoryBuilder()
    mem = lambda address: (7, address)
    b.instruction(0x149, 2, 0)
    b.glk(0x23, [0, 0, 0, 3, 0], mem(0x800))
    b.glk(0x2f, [mem(0x800)])
    b.glk(0xd2, [mem(0x800)])
    b.glk(0xc0, [0x804])
    b.instruction(0x40, 1, mem(0x814))
    b.instruction(0x24, mem(0x80c), ord('s'), 'open')
    b.instruction(0x40, 2, mem(0x814))
    b.labels['open'] = len(b.code)
    b.glk(0x62, [1, mem(0x814), 0], mem(0x818))
    b.glk(0x42, [mem(0x818), mem(0x814), 0], mem(0x81c))
    b.instruction(0x24, mem(0x814), 2, 'restore')
    b.instruction(0x178, 16, mem(0x824))
    b.instruction(0x4f, 0x830, 0, 1)  # bit-array write
    b.instruction(0x123, mem(0x81c), mem(0x820))
    b.text('GLULX-RESULT:')
    b.instruction(0x71, mem(0x820))
    b.instruction(0x70, 10)
    # Check double stack order using numtod -> dtonumz.
    b.instruction(0x200, -1234567, (8, 0), (8, 0))
    b.instruction(0x201, (8, 0), (8, 0), mem(0x828))
    b.text('DOUBLE:')
    b.instruction(0x71, mem(0x828))
    b.instruction(0x70, 10)
    b.instruction(0x120)
    b.labels['restore'] = len(b.code)
    b.instruction(0x124, mem(0x81c), mem(0x820))
    b.text('RESTORE-FAILED\n')
    b.instruction(0x120)
    return b.finish()


def command_for(executable, image, candidate, strict_glk=False):
    command = [str(pathlib.Path(executable).resolve())]
    if candidate:
        command.append('--headless')
        if strict_glk:
            command.append('--strict-glk')
    else:
        command.extend(['-q', '-u'])
    command.append(str(image))
    return command


def run_process(executable, image, input_text, candidate, cwd=None, timeout=20, strict_glk=False):
    command = command_for(executable, image, candidate, strict_glk)
    result = subprocess.run(command, input=input_text, text=True, capture_output=True, timeout=timeout, cwd=cwd)
    if result.returncode:
        raise AssertionError(f'{command}: {result.stderr}\n{result.stdout}')
    return result.stdout


def run(executable, image, save, action, candidate, cwd=None, timeout=20, strict_glk=False):
    return run_process(
        executable,
        image,
        f'{action}\n{save}\n',
        candidate,
        cwd=cwd,
        timeout=timeout,
        strict_glk=strict_glk,
    )


def normalize_output(output):
    output = output.replace('\r\n', '\n')
    return re.sub(r'Interpreter version [^ /\n]+', 'Interpreter version X', output)


def engine_specs(args):
    specs = {
        'Glulxe': (args.reference, False),
        'glulx-rs': (args.candidate, True),
    }
    if args.git:
        specs['Git'] = (args.git, False)
    return specs


def run_engine(specs, name, image, save, action, cwd=None, timeout=20, strict_glk=False):
    executable, candidate = specs[name]
    return run(
        executable,
        image,
        save,
        action,
        candidate,
        cwd=cwd,
        timeout=timeout,
        strict_glk=strict_glk,
    )


def run_engine_input(specs, name, image, input_text, cwd=None, timeout=20, strict_glk=False):
    executable, candidate = specs[name]
    return run_process(
        executable,
        image,
        input_text,
        candidate,
        cwd=cwd,
        timeout=timeout,
        strict_glk=strict_glk,
    )


def compare_transcripts(outputs, context, strip=False):
    baseline = normalize_output(outputs['Glulxe'])
    if strip:
        baseline = baseline.strip()
    for name, output in outputs.items():
        actual = normalize_output(output)
        if strip:
            actual = actual.strip()
        if actual != baseline:
            raise AssertionError(f'{context}: transcript differs for {name}')


def acceleration_story():
    """Exercise every standard acceleration function, using synthetic Inform objects."""
    b = StoryBuilder()
    mem = lambda address: (7, address)
    function, string = 0x7100, 0x7200
    metaclass, object_class, routine_class, string_class = 0x8400, 0x8440, 0x8480, 0x84c0
    b.instruction(0x149, 2, 0)
    b.glk(0x23, [0, 0, 0, 3, 0], mem(0x8000))
    b.glk(0x2f, [mem(0x8000)])
    for index, value in enumerate([0x9000, 256, metaclass, object_class, routine_class, string_class, 0x9040, 7, 0xa000]):
        b.instruction(0x181, index, value)
    expected = []

    def result(value):
        b.text('ACCEL:')
        b.instruction(0x71, (8, 0))
        b.instruction(0x70, 10)
        expected.append(value)

    def case(index, obj, prop, value):
        b.instruction(0x180, index, function)
        b.instruction(0x162, function, obj, prop, (8, 0))
        result(value)

    for address, value in [(0, 0), (35, 0), (0xffffffff, 0), (0xc100, 0), (function, 2), (string, 3), (0x8200, 1)]:
        case(1, address, 0, value)
    for obj, cls, attributes, indices in [
        (0x8200, 0x8300, 7, (2, 3, 4, 5, 6, 7)),
        (0x8200, 0x8300, 7, (8, 9, 10, 11, 12, 13)),
        (0x8500, 0x8600, 11, (8, 9, 10, 11, 12, 13)),
    ]:
        table, address, length, ofclass, value, provides = indices
        b.instruction(0x181, 7, attributes)
        b.instruction(0x40, cls, mem(0x9000))
        b.instruction(0x40, cls, mem(0x9200))
        b.instruction(0x40, 0, mem(0x9040))
        for index, target, prop, answer in [
            (table, obj, 10, 0x880e), (address, obj, 10, 0x9210),
            (length, obj, 10, 8), (value, obj, 10, 123), (provides, obj, 10, 1),
            (address, obj, 20, 0), (length, obj, 20, 0), (provides, obj, 20, 0),
            (value, obj, 30, 999), (address, cls, 10, 0),
            (value, obj, 10 << 16, 789), (length, obj, 10 << 16, 4),
            (address, object_class, 10 << 16, 0), (provides, cls, 256, 1),
            (ofclass, obj, cls, 1), (ofclass, obj, object_class, 1),
            (ofclass, obj, metaclass, 0), (ofclass, cls, metaclass, 1),
            (ofclass, cls, object_class, 0), (ofclass, object_class, metaclass, 1),
            (ofclass, function, routine_class, 1), (ofclass, string, string_class, 1),
            (provides, function, 261, 1), (provides, function, 262, 0),
            (provides, string, 262, 1), (provides, string, 263, 1),
        ]:
            case(index, target, prop, answer)
        b.instruction(0x40, obj, mem(0x9040))
        case(value, obj, 20, 456)

    b.instruction(0x180, 1, function)
    b.instruction(0x160, function, (8, 0))
    result(0)
    b.instruction(0x163, function, string, 17, 19, (8, 0))
    result(3)
    b.instruction(0x40, string, (8, 0))
    b.instruction(0x30, function, 1, (8, 0))
    result(3)
    b.instruction(0x160, 0x7180, (8, 0))  # ordinary routine tailcalls accelerated function
    result(3)
    for replacement in [14, 0]:
        b.instruction(0x180, 1, function)
        b.instruction(0x180, replacement, function)
        b.instruction(0x161, function, string, (8, 0))
        result(99)
    b.instruction(0x180, 1, 0x7140)
    b.instruction(0x149, 1, 0x7140)
    b.instruction(0x72, string)  # callbacks must use acceleration, without recursion
    b.instruction(0x149, 2, 0)
    b.text('ACCEL-DONE\n')
    b.instruction(0x120)
    image = b.finish(ram_start=0x8000, ext_start=0xc000, end_mem=0xc100)
    assert len(b.code) + 0x40 < function

    def word(address, value):
        image[address:address+4] = u32(value)

    def prop(address, number, size, data, private=False):
        image[address:address+10] = struct.pack('>HHIH', number, size, data, int(private))

    image[function:function+6] = b'\xc1\0\0\x31\x01\x63'
    image[0x7140:0x7140+8] = b'\xc1\0\0\x70\x01!\x31\0'
    tail = StoryBuilder()
    tail.instruction(0x40, string, (8, 0))
    tail.instruction(0x34, function, 1)
    image[0x7180:0x7180+len(tail.code)] = tail.code
    image[string:string+5] = b'\xe0abc\0'
    for obj in [0x8200, 0x8300, metaclass, object_class, routine_class, string_class, 0x8500, 0x8600]:
        image[obj] = 0x70
    for obj, cls, attributes in [(0x8200, 0x8300, 7), (0x8500, 0x8600, 11)]:
        word(obj + 4 * (3 + attributes // 4), 0x8800)
        word(cls + 4 * (3 + attributes // 4), 0x8900)
        word(cls + 13 + attributes, metaclass)
    word(0x8800, 3)
    prop(0x8804, 2, 1, 0x9200)
    prop(0x880e, 10, 2, 0x9210)
    prop(0x8818, 20, 1, 0x9220, True)
    word(0x8900, 1)
    prop(0x8904, 10, 1, 0x9230)
    for address, value in [(0x9210, 123), (0x9220, 456), (0x9230, 789), (0xa078, 999)]:
        word(address, value)
    word(32, 0)
    word(32, sum(value[0] for value in struct.iter_unpack('>I', image)))
    return image, expected


def core_boundary_story():
    """Valid zero-length block operations and a long Huffman substring stream."""
    builder = StoryBuilder()
    mem = lambda address: (7, address)
    builder.instruction(0x149, 2, 0)
    builder.glk(0x23, [0, 0, 0, 3, 0], mem(0x800))
    builder.glk(0x2f, [mem(0x800)])
    # No bytes are accessed, so ROM and out-of-range addresses are harmless.
    for address in [0, 32, 0x800, 0x3000, 0xffffffff]:
        builder.instruction(0x170, 0, address)
        builder.instruction(0x171, 0, address, 0xffffffff)
        builder.instruction(0x171, 0, 0xffffffff, address)
    builder.text('ZERO-OK\n')
    builder.instruction(0x141, 0x900)
    builder.instruction(0x72, 0xa00)
    builder.text('\nSTRING-OK\n')
    builder.instruction(0x120)
    image = builder.finish(ram_start=0x800, ext_start=0x3000, end_mem=0x3000)

    def word(address, value):
        image[address:address + 4] = u32(value)

    # Bit0 emits the substring "A"; bit1 terminates the encoded string.
    word(0x900, 0x61)
    word(0x904, 3)
    word(0x908, 0x910)
    image[0x910] = 0
    word(0x911, 0x920)
    word(0x915, 0x960)
    image[0x920:0x923] = b'\x03A\0'
    image[0x960] = 1
    image[0xa00] = 0xe1
    image[0xa01 + 5000] = 1
    word(32, 0)
    word(32, sum(value[0] for value in struct.iter_unpack('>I', image)))
    return image, 'ZERO-OK\n' + 'A' * 40_000 + '\nSTRING-OK\n'


def shared_stream_story():
    """Two ReadWrite handles share bytes while marks/counts stay independent."""
    b = StoryBuilder()
    mem = lambda address: (7, address)
    b.instruction(0x149, 2, 0)
    b.glk(0x23, [0, 0, 0, 3, 0], mem(0x800))
    b.glk(0x2f, [mem(0x800)])
    b.glk(0x61, [0, 0xa00, 0], mem(0x804))
    b.glk(0x42, [mem(0x804), 3, 0], mem(0x808))
    b.glk(0x42, [mem(0x804), 3, 0], mem(0x80c))
    b.glk(0x81, [mem(0x808), ord('X')])
    b.glk(0x44, [mem(0x808), 0])
    b.glk(0x45, [mem(0x80c), 0, 0])
    b.glk(0x90, [mem(0x80c)], mem(0x810))
    b.text('SHARED:')
    b.instruction(0x70, mem(0x810))
    b.instruction(0x70, 10)
    b.glk(0x45, [mem(0x80c), 1, 0])
    b.glk(0x81, [mem(0x80c), ord('Y')])
    b.glk(0x44, [mem(0x80c), 0x814])
    b.text('READCOUNT:')
    b.instruction(0x71, mem(0x814))
    b.text(' WRITECOUNT:')
    b.instruction(0x71, mem(0x818))
    b.instruction(0x70, 10)
    b.glk(0x42, [mem(0x804), 2, 0], mem(0x808))
    b.glk(0x92, [mem(0x808), 0x820, 3])
    b.text('DATA:')
    b.glk(0x84, [0x820, 3])
    b.instruction(0x70, 10)
    b.glk(0x44, [mem(0x808), 0])
    b.instruction(0x120)
    image = b.finish()
    name = b'\xe0sharedfile\0'
    image[0xa00:0xa00 + len(name)] = name
    image[32:36] = bytes(4)
    image[32:36] = u32(sum(word[0] for word in struct.iter_unpack('>I', image)))
    return image, 'SHARED:X\nREADCOUNT:1 WRITECOUNT:1\nDATA:XYC\n'


def unknown_selector_story():
    b = StoryBuilder()
    b.glk(0x7fff, [], (7, 0x800))
    b.text('UNKNOWN-SELECTOR-OK\n')
    b.instruction(0x120)
    return b.finish()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--reference', type=pathlib.Path, required=True)
    parser.add_argument('--candidate', type=pathlib.Path, required=True)
    parser.add_argument('--git', type=pathlib.Path, help='optional David Kinder Git executable; invoked with -q -u')
    parser.add_argument('--strict-glk', action='store_true', help='make the Rust candidate fail on unknown Glk selectors')
    parser.add_argument('--fixtures', type=pathlib.Path, help='Directory containing official Glulxercise, Unicode, resource-stream, and Adventure fixtures')
    parser.add_argument(
        '--route',
        action='append',
        nargs=2,
        type=pathlib.Path,
        metavar=('STORY', 'COMMAND_FILE'),
        help='run a command script against every selected engine; {save} expands to a private save path',
    )
    args = parser.parse_args()
    specs = engine_specs(args)
    with tempfile.TemporaryDirectory(prefix='glulx-conformance-') as directory:
        root = pathlib.Path(directory)
        image = root / 'interop.ulx'
        image.write_bytes(story())
        pairs = [('glulx-rs', 'Glulxe'), ('Glulxe', 'glulx-rs')]
        if args.git:
            pairs.extend([('Git', 'glulx-rs'), ('glulx-rs', 'Git'), ('Glulxe', 'Git'), ('Git', 'Glulxe')])
        for writer, reader in pairs:
            save = root / f'{writer.lower()}-to-{reader.lower()}.glksave'
            output = run_engine(specs, writer, image, save, 's', strict_glk=args.strict_glk)
            assert 'GLULX-RESULT:0' in output, output
            assert 'DOUBLE:-1234567' in output, output
            output = run_engine(specs, reader, image, save, 'r', strict_glk=args.strict_glk)
            assert 'GLULX-RESULT:-1' in output, output
            assert 'DOUBLE:-1234567' in output, output
            print(f'PASS {writer} -> {reader}: save continuation, heap chunk, double stack order')

        image = root / 'acceleration.ulx'
        data, expected = acceleration_story()
        image.write_bytes(data)
        expected_text = ''.join(f'ACCEL:{value}\n' for value in expected) + 'ACCEL-DONE\n'
        outputs = {}
        for name in specs:
            output = run_engine(specs, name, image, root / 'unused', '', strict_glk=args.strict_glk)
            # CheapGlk inserts an initial newline when the main window opens.
            assert normalize_output(output).strip() == normalize_output(expected_text).strip(), (name, output, expected_text)
            outputs[name] = output
        compare_transcripts(outputs, 'Acceleration')
        print(f'PASS acceleration: all 13 functions, {len(expected)} result checks, exact reference transcript')

        image = root / 'core-boundaries.ulx'
        data, expected = core_boundary_story()
        image.write_bytes(data)
        outputs = {}
        for name in specs:
            output = run_engine(specs, name, image, root / 'unused', '', strict_glk=args.strict_glk)
            assert normalize_output(output).strip() == normalize_output(expected).strip(), f'Core boundary output mismatch: engine={name}'
            outputs[name] = output
        compare_transcripts(outputs, 'Core boundaries')
        print('PASS core boundaries: zero-length memory operations and 40000 Huffman substrings, exact reference transcript')

        image = root / 'shared-streams.ulx'
        data, expected = shared_stream_story()
        image.write_bytes(data)
        for name in specs:
            file = root / 'sharedfile.glkdata'
            file.write_bytes(b'ABC')
            output = run_engine(specs, name, image, root / 'unused', '', cwd=root, strict_glk=args.strict_glk)
            assert normalize_output(output).strip() == normalize_output(expected).strip(), ('Shared stream transcript differs', name, output)
            assert file.read_bytes() == b'XYC', ('Shared file contents differ', name, file.read_bytes())
        print('PASS shared file streams: cross-handle reads, independent counts, and final file bytes match reference')

        if args.strict_glk:
            image = root / 'unknown-selector.ulx'
            image.write_bytes(unknown_selector_story())
            try:
                run_engine_input(specs, 'glulx-rs', image, '', strict_glk=True)
            except AssertionError as error:
                assert 'unsupported Glk selector' in str(error), error
                print('PASS strict Glk: unknown selector fails at the Rust VM boundary')
            else:
                raise AssertionError('strict Glk mode accepted an unknown selector')

    if args.fixtures:
        for fixture, commands in [('glulxercise.ulx','all\nallfloat\nalldouble\nquit\n'),('unicasetest.ulx','all\nquit\n'),('resstreamtest.gblorb','quit\n')]:
            file=args.fixtures / fixture
            outputs = {}
            for name in specs:
                output = run_engine_input(specs, name, file, commands, timeout=60, strict_glk=args.strict_glk)
                outputs[name] = output
                if 'FAIL' in output or 'tests failed' in output:
                    with tempfile.NamedTemporaryFile(mode='w', prefix=f'{fixture}-', suffix='.log', delete=False) as log:
                        log.write(output)
                        saved = log.name
                    failures = '\n'.join(line for line in output.splitlines() if 'FAIL' in line or 'tests failed' in line)
                    raise AssertionError(f'{fixture} ({name}): {failures}\nFull transcript: {saved}')
            candidate_output = outputs['glulx-rs']
            if fixture=='glulxercise.ulx':
                for name, output in outputs.items():
                    assert output.count('All tests passed.')==3, (fixture, name, output)
                print(f'PASS {fixture}: {candidate_output.count("Passed.")} passing sections for {", ".join(outputs)}')
            else:
                compare_transcripts(outputs, fixture)
                print(f'PASS {fixture}: exact normalized reference transcript for {", ".join(outputs)}')
        adventure=args.fixtures/'glulx-advent.ulx'
        if adventure.exists():
            with tempfile.TemporaryDirectory(prefix='glulx-adventure-') as directory:
                for writer, reader in pairs:
                    save=pathlib.Path(directory)/f'{writer.lower()}-to-{reader.lower()}.glksave'
                    output=run_engine_input(specs, writer, adventure, 'north\nsave\n'+str(save)+'\nquit\ny\n', timeout=30, strict_glk=args.strict_glk)
                    assert save.exists(),output
                    output=run_engine_input(specs, reader, adventure, 'restore\n'+str(save)+'\nlook\nquit\ny\n', timeout=30, strict_glk=args.strict_glk)
                    assert 'In Forest' in output and 'Restore failed' not in output,output
                    print(f'PASS Adventure {writer} -> {reader}')

    for index, (story_path, command_path) in enumerate(args.route or [], 1):
        story_path = story_path.resolve(strict=True)
        command_path = command_path.resolve(strict=True)
        command_text = command_path.read_text(encoding='utf-8')
        with tempfile.TemporaryDirectory(prefix='glulx-route-') as directory:
            outputs = {}
            for name in specs:
                save = pathlib.Path(directory) / f'{index}-{name.lower()}.glksave'
                input_text = command_text.replace('{save}', str(save))
                outputs[name] = run_engine_input(
                    specs,
                    name,
                    story_path,
                    input_text,
                    timeout=120,
                    strict_glk=args.strict_glk,
                )
            compare_transcripts(outputs, f'route {story_path.name}')
            print(f'PASS route {story_path.name}: {", ".join(outputs)}')


if __name__ == '__main__':
    main()
