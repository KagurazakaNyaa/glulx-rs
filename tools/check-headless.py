#!/usr/bin/env python3
"""Exercise the portable headless pipe and strict Glk error paths."""

import argparse
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile


def reference_tools():
    path = Path(__file__).with_name("check-reference.py")
    spec = importlib.util.spec_from_file_location("check_reference", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def normal_story(reference):
    builder = reference.StoryBuilder()
    memory = lambda address: (7, address)
    builder.instruction(0x149, 2, 0)
    builder.glk(0x23, [0, 0, 0, 3, 0], memory(0x800))
    builder.glk(0x2f, [memory(0x800)])
    builder.glk(0xd6, [1])
    builder.glk(0xc0, [], memory(0x804))
    builder.text("HEADLESS-OK\n")
    builder.instruction(0x120)
    return builder.finish()


def run(candidate, image, strict, trace=None):
    command = [str(candidate), "--headless", "--no-auto-resources"]
    if strict:
        command.append("--strict-glk")
    if trace:
        command.extend(["--trace-events", str(trace)])
    command.append(str(image))
    return subprocess.run(command, input="", text=True, capture_output=True, timeout=30)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--candidate", type=Path, default=Path("target/debug/glulx-rs"))
    args = parser.parse_args()
    candidate = args.candidate.resolve(strict=True)
    reference = reference_tools()
    with tempfile.TemporaryDirectory(prefix="glulx-headless-") as directory:
        root = Path(directory)
        image = root / "headless.ulx"
        image.write_bytes(normal_story(reference))
        trace = root / "events.json"
        result = run(candidate, image, True, trace)
        assert result.returncode == 0, (result.returncode, result.stdout, result.stderr)
        assert result.stdout == "HEADLESS-OK\n", repr(result.stdout)
        assert json.loads(trace.read_text(encoding="utf-8")) == [[1, 0, 0, 0]]
        print("PASS headless pipe and strict Glk mode")

        image = root / "unknown-selector.ulx"
        image.write_bytes(reference.unknown_selector_story())
        result = run(candidate, image, True)
        assert result.returncode != 0, result.stdout
        assert "unsupported Glk selector" in result.stderr, result.stderr
        print("PASS strict Glk unknown-selector failure")


if __name__ == "__main__":
    main()
