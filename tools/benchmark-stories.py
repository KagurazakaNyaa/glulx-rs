#!/usr/bin/env python3
"""Measure real story startup and resident memory without modifying story files."""

import argparse
import json
from pathlib import Path
import subprocess
import tempfile
import time


def memory_status(pid):
    try:
        values = {}
        for line in Path(f"/proc/{pid}/status").read_text().splitlines():
            if line.startswith(("VmRSS:", "VmHWM:")):
                values[line.split(":", 1)[0]] = int(line.split()[1])
        return values
    except (FileNotFoundError, PermissionError):
        return {}


def numeric(value):
    try:
        return int(value)
    except ValueError:
        try:
            return float(value)
        except ValueError:
            return value


def diagnostics_summary(path):
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except (FileNotFoundError, PermissionError, UnicodeDecodeError):
        return {"heartbeat_count": 0, "slow_stages": [], "heartbeats": []}

    heartbeats = []
    slow_stages = []
    selected = {
        "stage",
        "stage_ms",
        "frames",
        "slices",
        "interval_ms",
        "vm_ms",
        "ui_ms",
        "frame_delta",
        "slice_delta",
        "timings",
        "vm",
        "pc",
        "instructions",
        "polls",
        "poll_yields",
        "decode_hits",
        "decode_misses",
    }
    for line in lines:
        if "] heartbeat " in line:
            fields = {}
            payload = line.split("] heartbeat ", 1)[1]
            for token in payload.split():
                if "=" in token:
                    key, value = token.split("=", 1)
                    if key in selected:
                        fields[key] = numeric(value)
            heartbeats.append(fields)
        elif "] slow " in line:
            slow_stages.append(line.split("] ", 1)[1])
    return {
        "heartbeat_count": len(heartbeats),
        "slow_stages": slow_stages,
        "heartbeats": heartbeats,
    }


def benchmark(candidate, story, command, hold_seconds):
    with tempfile.TemporaryDirectory(prefix="glulx-real-benchmark-") as workdir:
        diagnostics = Path(workdir) / "diagnostics.log"
        started = time.perf_counter()
        process = subprocess.Popen(
            [str(candidate), "--headless", "--diagnostics", str(diagnostics), str(story)],
            cwd=workdir,
            stdin=subprocess.PIPE,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            text=True,
        )
        assert process.stdin is not None
        process.stdin.write(command)
        process.stdin.flush()
        peak = {"VmRSS": 0, "VmHWM": 0}
        deadline = started + hold_seconds
        while process.poll() is None and time.perf_counter() < deadline:
            for key, value in memory_status(process.pid).items():
                peak[key] = max(peak[key], value)
            time.sleep(0.02)
        termination = None
        if process.poll() is None:
            termination = "sample-timeout"
            process.terminate()
            try:
                process.wait(timeout=2)
            except subprocess.TimeoutExpired:
                termination = "killed-after-timeout"
                process.kill()
                process.wait()
        else:
            termination = "story-exited"
        profile = diagnostics_summary(diagnostics)
        process.stdin.close()
        return {
            "story": story.name,
            "story_bytes": story.stat().st_size,
            "elapsed_ms": round((time.perf_counter() - started) * 1000, 3),
            "peak_rss_kb": peak["VmRSS"] or None,
            "peak_hwm_kb": peak["VmHWM"] or None,
            "returncode": process.returncode,
            "termination": termination,
            "diagnostics": profile,
        }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("stories", nargs="+", type=Path)
    parser.add_argument("--candidate", type=Path, default=Path("target/release/glulx-rs"))
    parser.add_argument("--command", default="yes\n")
    parser.add_argument(
        "--command-file",
        type=Path,
        help="read a multiline input script from this file; overrides --command",
    )
    parser.add_argument("--hold-seconds", type=float, default=3.0)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if args.command_file:
        args.command = args.command_file.read_text(encoding="utf-8")
    candidate = args.candidate.resolve()
    root = Path(__file__).resolve().parents[1]
    dirty = bool(
        subprocess.check_output(
            ["git", "status", "--porcelain"], cwd=root, text=True
        ).strip()
    )
    records = [
        benchmark(candidate, story.resolve(), args.command, args.hold_seconds)
        for story in args.stories
    ]
    evidence = {
        "candidate": candidate.name,
        "commit": subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=root, text=True
        ).strip(),
        "worktree_dirty": dirty,
        "command": args.command,
        "hold_seconds": args.hold_seconds,
        "benchmarks": records,
    }
    encoded = json.dumps(evidence, indent=2, sort_keys=True) + "\n"
    if args.output:
        args.output.write_text(encoded, encoding="utf-8")
        print(f"Wrote {args.output}")
    else:
        print(encoded, end="")


if __name__ == "__main__":
    main()
