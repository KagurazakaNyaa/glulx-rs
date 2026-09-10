#!/usr/bin/env python3
"""Run the release microbenchmarks and write reproducible JSON evidence."""

import argparse
import json
import platform
from pathlib import Path
import re
import subprocess
import sys


BENCHMARK = re.compile(r"^BENCHMARK\s+(?P<fields>.+)$")


def command_output(command, root):
    try:
        return subprocess.check_output(command, cwd=root, text=True, stderr=subprocess.STDOUT).strip()
    except (OSError, subprocess.CalledProcessError):
        return None


def parse_benchmarks(output):
    records = []
    for line in output.splitlines():
        match = BENCHMARK.match(line.strip())
        if not match:
            continue
        record = {}
        for field in match.group("fields").split():
            key, value = field.split("=", 1)
            try:
                record[key] = int(value)
            except ValueError:
                try:
                    record[key] = float(value)
                except ValueError:
                    record[key] = value
        records.append(record)
    return records


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--output",
        type=Path,
        help="write JSON evidence to this path instead of stdout",
    )
    parser.add_argument("--cargo", default="cargo", help="cargo executable")
    args = parser.parse_args()

    root = Path(__file__).resolve().parents[1]
    command = [
        args.cargo,
        "test",
        "--release",
        "--lib",
        "benchmark_",
        "--",
        "--ignored",
        "--nocapture",
        "--test-threads=1",
    ]
    result = subprocess.run(command, cwd=root, text=True, capture_output=True)
    output = result.stdout + result.stderr
    if result.returncode != 0:
        sys.stderr.write(output)
        return result.returncode

    benchmarks = parse_benchmarks(output)
    if not benchmarks:
        sys.stderr.write("benchmark command produced no BENCHMARK records\n")
        sys.stderr.write(output)
        return 1

    evidence = {
        "commit": command_output(["git", "rev-parse", "HEAD"], root),
        "platform": platform.platform(),
        "python": platform.python_version(),
        "rustc": command_output(["rustc", "-Vv"], root),
        "command": command,
        "benchmarks": benchmarks,
    }
    encoded = json.dumps(evidence, indent=2, sort_keys=True) + "\n"
    if args.output:
        args.output.write_text(encoded, encoding="utf-8")
        print(f"Wrote {args.output}")
    else:
        print(encoded, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
