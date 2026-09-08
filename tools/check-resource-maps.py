#!/usr/bin/env python3
"""Split a Blorb and compare bundled, explicit, discovered and loose resources.

Use the official resstreamtest.gblorb as --fixture for the transcript checks.
--split-only also prepares other Blorb stories for GUI/media acceptance.
"""

import argparse
import pathlib
import re
import struct
import subprocess
import tempfile


def u32(value):
    return struct.pack(">I", value)


def read_u32(data, offset):
    return struct.unpack_from(">I", data, offset)[0]


def split_archive(source, output):
    assert source[:4] == b"FORM" and source[8:12] == b"IFRS", "Expected a Blorb"
    chunks = []
    position = 12
    while position < read_u32(source, 4) + 8:
        length = read_u32(source, position + 4)
        chunks.append((position, source[position:position + 4],
                       source[position + 8:position + 8 + length]))
        position += 8 + length + length % 2
    index = next(data for _, tag, data in chunks if tag == b"RIdx")
    entries = [(index[pos:pos + 4], read_u32(index, pos + 4), read_u32(index, pos + 8))
               for pos in range(4, 4 + read_u32(index, 0) * 12, 12)]
    executable = next((offset for usage, number, offset in entries
                       if usage == b"Exec" and number == 0), None)
    if executable is None:
        # Some existing generated fixtures use the player's compatible
        # unindexed-GLUL loading path.
        executable = next(offset for offset, tag, _ in chunks if tag == b"GLUL")
    image = next(data for offset, tag, data in chunks
                 if offset == executable and tag == b"GLUL")
    executable_offsets = {offset for usage, _, offset in entries if usage == b"Exec"}
    executable_offsets.add(executable)
    entries = [entry for entry in entries if entry[0] != b"Exec"]
    kept = [(offset, tag, data) for offset, tag, data in chunks
            if offset not in executable_offsets and tag != b"RIdx"]
    if not any(tag == b"IFhd" for _, tag, _ in kept):
        kept.append((-1, b"IFhd", image[:128]))
    archive = bytearray(b"FORM\0\0\0\0IFRSRIdx" + u32(4 + 12 * len(entries))
                        + u32(len(entries)) + bytes(12 * len(entries)))
    offsets = {}
    for original, tag, data in kept:
        offsets[original] = len(archive)
        archive.extend(tag + u32(len(data)) + data + bytes(len(data) % 2))
    for index, (usage, number, original) in enumerate(entries):
        position = 24 + index * 12
        archive[position:position + 12] = usage + u32(number) + u32(offsets[original])
    archive[4:8] = u32(len(archive) - 8)

    output.mkdir(parents=True, exist_ok=True)
    (output / "story.ulx").write_bytes(image)
    (output / "story.blorb").write_bytes(archive)
    loose = output / "loose"
    loose.mkdir(exist_ok=True)
    lookup = {offset: (tag, data) for offset, tag, data in kept}
    suffixes = {
        b"Pict": ("PIC", {b"PNG ": ".png", b"JPEG": ".jpeg"}),
        b"Snd ": ("SND", {b"FORM": ".aiff", b"MOD ": ".mod", b"SONG": ".song",
                          b"OGGV": ".ogg", b"MP3 ": ".mp3"}),
        b"Data": ("DATA", {b"TEXT": ".txt", b"BINA": ".bin", b"FORM": ".form"}),
    }
    for usage, number, offset in entries:
        tag, data = lookup[offset]
        prefix, types = suffixes[usage]
        if tag == b"FORM":
            data = tag + u32(len(data)) + data
        (loose / f"{prefix}{number}{types[tag]}").write_bytes(data)
    names = {b"IFhd": "IDENT", b"IFmd": "METADATA.xml", b"Fspc": "FRONTIS",
             b"RDes": "RESDESC", b"Plte": "PALETTE", b"RelN": "RELEASE",
             b"Reso": "RESOL", b"APal": "ADAPTPAL", b"Loop": "LOOPING"}
    for _, tag, data in kept:
        if tag in names:
            (loose / names[tag]).write_bytes(data)
    return len(entries)


def normalize(text):
    return re.sub(r"Interpreter version [^ /]+", "Interpreter version X", text)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fixture", type=pathlib.Path, required=True)
    parser.add_argument("--candidate", type=pathlib.Path, default=pathlib.Path("target/debug/glulx-rs"))
    parser.add_argument("--reference", type=pathlib.Path, help="Optional Glulxe/CheapGlk executable")
    parser.add_argument("--output", type=pathlib.Path)
    parser.add_argument("--split-only", action="store_true")
    arguments = parser.parse_args()
    fixture = arguments.fixture.resolve()
    output = (arguments.output or pathlib.Path(tempfile.mkdtemp(prefix="glulx-resource-maps-"))).resolve()
    # Use a fresh child so rerunning with another fixture cannot leave stale
    # loose resources which silently change the archive being tested.
    output.mkdir(parents=True, exist_ok=True)
    output = pathlib.Path(tempfile.mkdtemp(prefix="resources-", dir=output))
    count = split_archive(fixture.read_bytes(), output)
    print(f"Split {count} resources into {output}")
    if arguments.split_only:
        return
    candidate = str(arguments.candidate.resolve())
    transcripts = {}
    for name, options in {
        "bundled": [str(fixture), "--no-auto-resources"],
        "explicit": [str(output / "story.ulx"), "--resources", str(output / "story.blorb")],
        "automatic": [str(output / "story.ulx")],
        "loose": [str(output / "story.ulx"), "--resources", str(output / "loose")],
    }.items():
        result = subprocess.run([candidate, "--headless", *options], input="quit\n",
                                text=True, capture_output=True, timeout=30, check=True)
        (output / f"{name}.txt").write_text(result.stdout)
        transcripts[name] = normalize(result.stdout)
        assert transcripts[name] == transcripts["bundled"], f"{name} differs; see {output}"
        print(f"PASS {name}: exact resource-stream transcript ({len(result.stdout)} characters)")
    if arguments.reference:
        result = subprocess.run([str(arguments.reference.resolve()), "-q", "-u", str(fixture)],
                                input="quit\n", text=True, capture_output=True, timeout=30, check=True)
        (output / "reference.txt").write_text(result.stdout)
        assert normalize(result.stdout) == transcripts["bundled"], f"Reference differs; see {output}"
        print("PASS all resource arrangements match the normalized Glulxe transcript")


if __name__ == "__main__":
    main()
