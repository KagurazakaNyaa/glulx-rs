#!/usr/bin/env python3
"""Generate original AIFF/Vorbis/MP3 audio and verify streaming replay/resume.

Requires cargo, ffmpeg and rustc. Generated assets stay in a temporary directory.
"""

import json
import os
import pathlib
import shutil
import subprocess
import tempfile


def main():
    root = pathlib.Path(__file__).resolve().parents[1]
    for command in ["cargo", "ffmpeg", "rustc"]:
        if shutil.which(command) is None:
            raise SystemExit(f"{command} is required")
    # Cargo identifies the exact feature-selected artifacts, even when older
    # variants of these libraries remain in target/debug/deps.
    build = subprocess.run(
        ["cargo", "build", "--lib", "--message-format=json"], cwd=root,
        env={**os.environ, "RUSTC_WRAPPER": ""}, check=True, capture_output=True, text=True,
    )
    libraries = {}
    for line in build.stdout.splitlines():
        artifact = json.loads(line)
        if artifact.get("reason") == "compiler-artifact":
            name = artifact["target"]["name"]
            if name in ("rodio", "symphonia"):
                libraries[name] = next(path for path in artifact["filenames"] if path.endswith(".rlib"))
    source_path = json.dumps(str(root / "src/vm/sound/sampled.rs"), ensure_ascii=False)
    source = f"#[path={source_path}] mod sampled;\n" + r'''
use rodio::Source;
fn main() {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    for arguments in arguments.chunks_exact(2) {
        let path = &arguments[0];
        let millis: u64 = arguments[1].parse().unwrap();
        let bytes = std::fs::read(path).unwrap();
        let source = sampled::SampledSource::new(&bytes, 1, 0).unwrap();
        let (rate, channels) = (source.sample_rate(), source.channels());
        assert_eq!((rate, channels), (44100, 2), "format: {path}");
        let duration = source.total_duration().expect("known generated duration");
        assert!(duration.abs_diff(std::time::Duration::from_millis(millis)) <= std::time::Duration::from_millis(1), "duration: {path}: {duration:?}");
        let once: Vec<_> = source.collect();
        assert_eq!(once.len() as u64, millis * u64::from(rate) / 1000 * u64::from(channels), "codec padding: {path}");
        assert!(once.iter().any(|sample| sample.abs() > 0.01));
        let twice: Vec<_> = sampled::SampledSource::new(&bytes, 2, 0).unwrap().collect();
        assert_eq!(twice, once.repeat(2), "repeat: {path}");
        let skipped = (5 * rate as usize / 1000) * channels as usize;
        let resumed: Vec<_> = sampled::SampledSource::new(&bytes, 2, 5).unwrap().collect();
        assert_eq!(resumed, twice[skipped..], "resume: {path}");
        let infinite: Vec<_> = sampled::SampledSource::new(&bytes, u32::MAX, 0).unwrap().take(twice.len()).collect();
        assert_eq!(infinite, twice, "infinite: {path}");
        let converted: Vec<f32> = rodio::source::UniformSourceIterator::new(sampled::SampledSource::new(&bytes, 2, 0).unwrap(), channels, rate).collect();
        assert_eq!(converted, twice, "playback conversion: {path}");
        println!("{}: {} samples, {} Hz, {} channels; duration/repeats/resume/conversion passed", std::path::Path::new(path).file_name().unwrap().to_string_lossy(), once.len(), rate, channels);
    }
}
'''
    with tempfile.TemporaryDirectory(prefix="glulx-audio-") as directory:
        directory = pathlib.Path(directory)
        checker = directory / "check.rs"
        checker.write_text(source)
        executable = directory / "check"
        library_args = [arg for name, path in libraries.items() for arg in ["--extern", f"{name}={path}"]]
        subprocess.run([
            "rustc", "--edition=2024", str(checker), "-L",
            f"dependency={root / 'target/debug/deps'}", *library_args, "-o", str(executable),
        ], check=True)
        outputs = []
        for extension, codec in [("aiff", "pcm_s16be"), ("ogg", "libvorbis"), ("mp3", "libmp3lame")]:
            # Short Vorbis audio fits on one Ogg page; longer audio also
            # exercises packet trimming across page boundaries.
            for millis in [250, 1250]:
                output = directory / f"tone-{millis}ms.{extension}"
                subprocess.run([
                    "ffmpeg", "-loglevel", "error", "-f", "lavfi", "-i",
                    f"sine=frequency=440:sample_rate=44100:duration={millis / 1000}",
                    "-ac", "2", "-c:a", codec, str(output),
                ], check=True)
                outputs.extend([str(output), str(millis)])
        subprocess.run([str(executable), *outputs], check=True)


if __name__ == "__main__":
    main()
