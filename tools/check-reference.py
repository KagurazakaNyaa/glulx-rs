#!/usr/bin/env python3
"""Reproducible Glulxe differential and bidirectional IFZS smoke test.

Usage: python3 tools/check-reference.py --reference /path/to/glulxe --candidate target/debug/glulx-rs
Requires Python 3; generates a synthetic story in a temporary directory.
"""
import argparse
import pathlib
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


def run(executable, image, save, action, candidate):
    command = [str(executable)] + (['--headless'] if candidate else ['-q', '-u']) + [str(image)]
    result = subprocess.run(command, input=f'{action}\n{save}\n', text=True, capture_output=True, timeout=20)
    if result.returncode:
        raise AssertionError(f'{command}: {result.stderr}\n{result.stdout}')
    return result.stdout


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


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--reference', type=pathlib.Path, required=True)
    parser.add_argument('--candidate', type=pathlib.Path, required=True)
    parser.add_argument('--fixtures', type=pathlib.Path, help='Directory containing official Glulxercise, Unicode, resource-stream, and Adventure fixtures')
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='glulx-conformance-') as directory:
        root = pathlib.Path(directory)
        image = root / 'interop.ulx'
        image.write_bytes(story())
        for writer, reader, name in [(True, False, 'Rust -> Glulxe'), (False, True, 'Glulxe -> Rust')]:
            save = root / ('rust.glksave' if writer else 'glulxe.glksave')
            output = run(args.candidate if writer else args.reference, image, save, 's', writer)
            assert 'GLULX-RESULT:0' in output, output
            assert 'DOUBLE:-1234567' in output, output
            output = run(args.candidate if reader else args.reference, image, save, 'r', reader)
            assert 'GLULX-RESULT:-1' in output, output
            assert 'DOUBLE:-1234567' in output, output
            print(f'PASS {name}: save continuation, heap chunk, double stack order')

        image = root / 'acceleration.ulx'
        data, expected = acceleration_story()
        image.write_bytes(data)
        expected_text = ''.join(f'ACCEL:{value}\n' for value in expected) + 'ACCEL-DONE\n'
        transcripts = []
        for candidate in [False, True]:
            executable = args.candidate if candidate else args.reference
            output = run(executable, image, root / 'unused', '', candidate)
            # CheapGlk inserts an initial newline when the main window opens.
            assert output.strip() == expected_text.strip(), (candidate, output, expected_text)
            transcripts.append(output)
        assert transcripts[0] == transcripts[1], 'Acceleration transcripts differ'
        print(f'PASS acceleration: all 13 functions, {len(expected)} result checks, exact reference transcript')

    if args.fixtures:
        import re
        for fixture, commands in [('glulxercise.ulx','all\nallfloat\nalldouble\nquit\n'),('unicasetest.ulx','all\nquit\n'),('resstreamtest.gblorb','quit\n')]:
            file=args.fixtures / fixture
            result=subprocess.run([str(args.candidate),'--headless',str(file)],input=commands,text=True,capture_output=True,timeout=60)
            assert result.returncode==0,(fixture,result.stderr)
            assert 'FAIL' not in result.stdout and 'tests failed' not in result.stdout,(fixture,result.stdout)
            if fixture=='glulxercise.ulx':
                assert result.stdout.count('All tests passed.')==3,result.stdout
                print(f'PASS {fixture}: {result.stdout.count("Passed.")} passing sections')
            else:
                reference=subprocess.run([str(args.reference),'-q','-u',str(file)],input=commands,text=True,capture_output=True,timeout=60)
                normalize=lambda output:re.sub(r'Interpreter version [^ /]+','Interpreter version X',output)
                assert reference.returncode==0,reference.stderr
                assert normalize(reference.stdout)==normalize(result.stdout),f'{fixture}: transcript differs'
                print(f'PASS {fixture}: exact normalized reference transcript')
        adventure=args.fixtures/'glulx-advent.ulx'
        if adventure.exists():
            with tempfile.TemporaryDirectory(prefix='glulx-adventure-') as directory:
                for writer,reader,name in [(True,False,'Rust -> Glulxe'),(False,True,'Glulxe -> Rust')]:
                    save=pathlib.Path(directory)/('rust.glksave' if writer else 'glulxe.glksave')
                    def play(candidate,commands):
                        command=[str(args.candidate),'--headless'] if candidate else [str(args.reference),'-q','-u']
                        result=subprocess.run(command+[str(adventure)],input=commands,text=True,capture_output=True,timeout=30)
                        assert result.returncode==0,result.stderr
                        return result.stdout
                    output=play(writer,'north\nsave\n'+str(save)+'\nquit\ny\n')
                    assert save.exists(),output
                    output=play(reader,'restore\n'+str(save)+'\nlook\nquit\ny\n')
                    assert 'In Forest' in output and 'Restore failed' not in output,output
                    print(f'PASS Adventure {name}')


if __name__ == '__main__':
    main()
