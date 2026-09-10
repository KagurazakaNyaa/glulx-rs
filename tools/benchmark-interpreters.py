#!/usr/bin/env python3
"""Measure one real-story route with Glulxe, Git and glulx-rs."""

import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import platform
import subprocess
import tempfile
import time


def reference_tools():
    path = Path(__file__).with_name("check-reference.py")
    spec = importlib.util.spec_from_file_location("check_reference", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def digest(value):
    return hashlib.sha256(value.encode()).hexdigest()


def text(value):
    return value.decode(errors="replace") if isinstance(value, bytes) else value


def run(engine, executable, candidate, story, input_text, workdir, timeout, strict_glk, trace_path=None):
    tools = reference_tools()
    command = tools.command_for(executable, story, candidate, strict_glk)
    if candidate and trace_path:
        command.insert(len(command) - 1, "--trace-events")
        command.insert(len(command) - 1, str(trace_path))
    started = time.perf_counter()
    try:
        result = subprocess.run(
            command,
            input=input_text,
            text=True,
            capture_output=True,
            cwd=workdir,
            timeout=timeout,
        )
        timed_out = False
        stdout = result.stdout
        stderr = result.stderr
        returncode = result.returncode
    except subprocess.TimeoutExpired as error:
        timed_out = True
        stdout = text(error.stdout or "")
        stderr = text(error.stderr or "")
        returncode = None
    record = {
        "engine": engine,
        "command": command,
        "elapsed_ms": round((time.perf_counter() - started) * 1000, 3),
        "returncode": returncode,
        "timed_out": timed_out,
        "stdout_bytes": len(stdout.encode()),
        "stderr_bytes": len(stderr.encode()),
        "stdout_sha256": digest(stdout),
        "stderr_sha256": digest(stderr),
    }
    if trace_path and trace_path.is_file():
        trace = trace_path.read_bytes()
        record["event_count"] = len(json.loads(trace))
        record["event_trace_sha256"] = hashlib.sha256(trace).hexdigest()
    return record


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("story", type=Path)
    parser.add_argument("--reference", type=Path, required=True)
    parser.add_argument("--candidate", type=Path, default=Path("target/release/glulx-rs"))
    parser.add_argument("--git", type=Path, help="optional David Kinder Git executable")
    inputs = parser.add_mutually_exclusive_group(required=True)
    inputs.add_argument("--command", help="input script sent to every engine")
    inputs.add_argument("--command-file", type=Path, help="input script file sent to every engine")
    parser.add_argument("--repetitions", type=int, default=1)
    parser.add_argument("--timeout", type=float, default=120.0)
    parser.add_argument("--strict-glk", action="store_true")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if args.repetitions < 1:
        parser.error("--repetitions must be positive")
    if args.timeout <= 0:
        parser.error("--timeout must be positive")

    story = args.story.resolve(strict=True)
    command_file = args.command_file.resolve(strict=True) if args.command_file else None
    command_text = command_file.read_text(encoding="utf-8") if command_file else args.command
    specs = [("Glulxe", args.reference, False), ("glulx-rs", args.candidate, True)]
    if args.git:
        specs.append(("Git", args.git, False))

    records = []
    for repetition in range(1, args.repetitions + 1):
        with tempfile.TemporaryDirectory(prefix="glulx-interpreter-benchmark-") as directory:
            for engine, executable, candidate in specs:
                save = Path(directory) / f"{engine.lower()}.glksave"
                trace = Path(directory) / f"{engine.lower()}-events.json" if candidate else None
                input_text = command_text.replace("{save}", str(save))
                record = run(
                    engine,
                    executable,
                    candidate,
                    story,
                    input_text,
                    directory,
                    args.timeout,
                    args.strict_glk,
                    trace,
                )
                record["repetition"] = repetition
                records.append(record)

    root = Path(__file__).resolve().parents[1]
    evidence = {
        "commit": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip(),
        "platform": platform.platform(),
        "python": platform.python_version(),
        "story": story.name,
        "story_bytes": story.stat().st_size,
        "command_sha256": digest(command_text),
        "repetitions": args.repetitions,
        "timeout_seconds": args.timeout,
        "strict_glk": args.strict_glk,
        "benchmarks": records,
    }
    encoded = json.dumps(evidence, indent=2, sort_keys=True) + "\n"
    if args.output:
        args.output.write_text(encoded, encoding="utf-8")
        print(f"Wrote {args.output}")
    else:
        print(encoded, end="")
    return int(any(record["returncode"] != 0 or record["timed_out"] for record in records))


if __name__ == "__main__":
    raise SystemExit(main())
