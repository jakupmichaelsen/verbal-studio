# AGENTS.md

VerbalStudio is a single-file Rust TUI (`src/main.rs`) with three Python helper scripts (`scripts/*.py`).

## Commands

| Action | Command |
|---|---|
| Check all | `cargo fmt --check && cargo test && cargo build && python3 -m py_compile scripts/*.py` |
| Run TUI | `cargo run -- --audio file.mp3 --assignment requirements.md` |
| Run TUI with auto-assess | `cargo run -- --audio file.mp3 --assignment requirements.md --auto-assess` |
| Run sample | `cargo run -- --srt examples/transcript.srt --assignment examples/assignment.md` |

## Python scripts and venv

- `scripts/transcribe.py` — calls OpenAI Whisper. Requires `OPENAI_API_KEY`.
- `scripts/auto_assess.py` — maps transcript evidence to requirements. Reads JSON from stdin.
- `scripts/generate_feedback.py` — turns reviewed notes into feedback markdown. Reads JSON from stdin.

`auto_assess.py` and `generate_feedback.py` auto-re-exec into `Python/.venv/bin/python` if it exists. `transcribe.py` does **not** — it runs with whatever `python3` is on PATH. Create `Python/.venv` with `openai>=1.0.0` installed, or install `openai` globally.

## Key behaviours

- **Transcript auto-discovery**: if `--srt` is omitted but `--audio` is given, looks for a same-name `.srt`. If missing, calls `scripts/transcribe.py` before the TUI opens.
- **Assignment parsing**: preferentially reads the `## Requirements Checklist` section (headings/bullets). Falls back to all non-empty lines.
- **Exported files** (`verbalstudio-assessment.json`, `verbalstudio-feedback.md`) are gitignored — they live in the working directory.
- **Audio playback** uses `rodio` inside the TUI process. Fails gracefully if no audio output is available.
- **Large audio** (>24 MB) is automatically compressed via `ffmpeg` before upload to Whisper. `ffmpeg` must be installed.
- **Default models**: `whisper-1` (transcription), `gpt-4.1-mini` (assessment/feedback). Override with `--assessment-model` or `OPENAI_ASSESSMENT_MODEL`.

## Architecture

Single binary, no external crates beyond `ratatui`, `crossterm`, `rodio`, `serde`/`serde_json`. All logic lives in `src/main.rs` (~1400 lines including tests). Python scripts are subprocess calls for OpenAI API access.

## Testing

Tests are inline in `src/main.rs` under `#[cfg(test)]`. Run with `cargo test`. Python scripts are syntax-validated with `py_compile` in CI; no Python test suite exists.
