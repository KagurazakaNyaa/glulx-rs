# Repository Instructions

## Documentation Paths

- Documentation must not contain host-specific absolute paths, private workspace paths, or user-specific paths.
- Use online URLs, repository-relative paths, or explicit placeholders such as `<story-dir>`, `<fixtures-dir>`, `<output-dir>`, and `<log-path>`.
- `/tmp/...` is allowed only when it clearly denotes an ephemeral generated fixture or output; it must not be presented as a required shared input location.
- Generic example paths such as `/path/to/...` are allowed as placeholders, but prefer named placeholders when they make the expected input clearer.
- Benchmark and validation commands must accept real story, fixture, and tool paths as command-line arguments. Never hardcode a developer machine's path in documentation or example commands.
- Before committing documentation, search for host-specific path patterns and run `git diff --check`.
