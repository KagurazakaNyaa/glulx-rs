#!/usr/bin/env python3
"""Check opcode dispatch and operand counts against the public Glulx spec.

Example:
  python3 tools/check-opcodes.py --spec /tmp/Glulx-Spec.md

The input is the official Markdown specification from
https://eblong.com/zarf/glulx/Glulx-Spec.md
This checks table completeness and instruction decoding, not opcode semantics;
use the Rust regressions and check-reference.py for execution coverage.
"""

import argparse
import csv
import pathlib
import re
import subprocess
import tempfile


def balanced_block(source, beginning):
    """Extract a Rust block; the audited functions contain no braces in strings."""
    start = source.index("{", beginning)
    depth = 1
    end = start + 1
    while depth:
        if source[end] == "{":
            depth += 1
        elif source[end] == "}":
            depth -= 1
        end += 1
    return source[beginning:end]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--spec", type=pathlib.Path, required=True)
    parser.add_argument("--source", type=pathlib.Path,
                        default=pathlib.Path(__file__).resolve().parents[1] / "src/vm.rs")
    parser.add_argument("--output", type=pathlib.Path, help="Write the per-opcode audit as TSV")
    args = parser.parse_args()
    specification = args.spec.read_text()
    source = args.source.read_text()
    opcodes = {int(number, 16): name for number, name in re.findall(
        r"^- (0x[0-9A-Fa-f]+): ([a-z][a-z0-9]*)$", specification, re.M)}
    if not opcodes:
        raise AssertionError("No opcode table found in specification")
    signatures = {}
    for block in re.findall(r"^```\n(.*?)^```", specification, re.M | re.S):
        for line in block.splitlines():
            match = re.fullmatch(r"([a-z][a-z0-9]*)((?: [LS][0-9]+)*)", line)
            if match and match[1] in opcodes.values():
                count = len(match[2].split())
                previous = signatures.setdefault(match[1], count)
                if previous != count:
                    raise AssertionError(f"Conflicting specification signatures for {match[1]}")

    function = balanced_block(source, source.index("fn operand_count("))
    with tempfile.TemporaryDirectory(prefix="glulx-opcodes-") as directory:
        directory = pathlib.Path(directory)
        rust = directory / "arity.rs"
        executable = directory / "arity"
        rust.write_text(function + '\nfn main() {\n'
                        + f'    for opcode in 0..={max(opcodes)} {{\n'
                        + '        if let Some(count) = operand_count(opcode) {\n'
                        + '            println!("{opcode}\\t{count}");\n'
                        + '        }\n    }\n}\n')
        subprocess.run(["rustc", "--edition=2024", str(rust), "-o", str(executable)], check=True)
        result = subprocess.run([str(executable)], check=True, capture_output=True, text=True)
    arities = dict(tuple(map(int, line.split())) for line in result.stdout.splitlines())

    step = balanced_block(source, source.index("fn step("))
    handlers = set()
    for arm in re.findall(r"^            (0x.*?)=>", step, re.M | re.S):
        for first, last in re.findall(r"(0x[0-9a-f]+)(?:\.\.=\s*(0x[0-9a-f]+))?", arm):
            first = int(first, 16)
            handlers.update(range(first, int(last, 16) + 1) if last else [first])

    rows, failures = [], []
    for number, name in sorted(opcodes.items()):
        expected = signatures.get(name)
        actual = arities.get(number)
        dispatch = number in handlers
        rows.append((f"0x{number:03x}", name, expected, actual, dispatch))
        if expected is None or expected != actual or not dispatch:
            failures.append(rows[-1])
    if set(arities) != set(opcodes):
        failures.append(("unexpected decoded opcodes", sorted(set(arities) - set(opcodes))))
    if set(handlers) != set(opcodes):
        failures.append(("unexpected dispatch opcodes", sorted(set(handlers) - set(opcodes))))
    if args.output:
        with args.output.open("w", newline="") as output:
            writer = csv.writer(output, delimiter="\t")
            writer.writerow(("opcode", "name", "spec_operands", "vm_operands", "dispatch"))
            writer.writerows(rows)
    if failures:
        raise AssertionError(f"Opcode table/decoding mismatches: {failures}")
    print(f"PASS: {len(opcodes)} specification opcodes have dispatch and matching operand counts.")
    print("Scope: table completeness and decoding arity; execution semantics require runtime tests.")


if __name__ == "__main__":
    main()
