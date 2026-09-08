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

    def finish(self):
        image = bytearray(0x1000)
        image[:4] = b'Glul'
        for offset, value in [(4, 0x30103), (8, 0x800), (12, 0x1000), (16, 0x1100), (20, 0x1000), (24, 0x40)]:
            image[offset:offset+4] = u32(value)
        for offset, label in self.fixups:
            self.code[offset:offset+4] = u32(self.labels[label] - (offset + 4) + 2)
        assert len(self.code) < 0x7c0
        image[0x40:0x40+len(self.code)] = self.code
        image[32:36] = u32(sum(struct.unpack('>1024I', image)))
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
