# VerbalStudio

Repository and deployment slug: `verbal-studio`.

Keyboard-first local assessment tool for oral feedback:

```text
assignment requirement -> transcript evidence -> teacher note -> feedback export
```

This prototype is a Rust terminal app built with `ratatui`. Pass an audio file,
and VerbalStudio will reuse a matching `.srt` or create one before the TUI opens.

## Requirements

- Rust toolchain
- Python 3
- `openai` Python package for transcription and AI assessment
- `OPENAI_API_KEY` in the shell for transcription, auto-assessment, or feedback generation
- `ffmpeg` for temporary compression of large audio files

Install the Python SDK if needed:

```bash
python3 -m pip install --upgrade openai
```

## Run

```bash
cargo run -- \
  --audio path/to/presentation.mp3 \
  --assignment path/to/requirements.md
```

Add `--auto-assess` to ask OpenAI to pre-link transcript evidence to the
requirements before the TUI opens:

```bash
cargo run -- path/to/presentation.mp3 \
  --assignment path/to/requirements.md \
  --auto-assess
```

The auto-assessment is a review starter, not a final grade. It suggests status,
teacher notes, and transcript evidence links that should be checked by a human.
Use `--assessment-model` or `OPENAI_ASSESSMENT_MODEL` to change the model.

After reviewing the evidence links and notes, press `f` in the TUI to generate
the feedback section. This uses the full assignment file, including the custom
feedback instructions, plus the current reviewed notes/evidence.

If `path/to/presentation.srt` already exists, it is loaded. If it does not exist,
VerbalStudio calls:

```bash
python3 scripts/transcribe.py path/to/presentation.mp3 \
  --format srt \
  --output path/to/presentation.srt
```

You can still provide an explicit transcript:

```bash
cargo run -- \
  --audio path/to/presentation.mp3 \
  --srt path/to/custom-transcript.srt \
  --assignment path/to/requirements.md
```

For mixed-language speech, leave `--language` unset or add a prompt:

```bash
cargo run -- presentation.mp3 \
  --assignment requirements.md \
  --prompt "The presentation is mostly English but may include accidental Danish words and names."
```

Try the sample data:

```bash
cargo run -- \
  --srt examples/transcript.srt \
  --assignment examples/assignment.md
```

## Keys

```text
Tab        switch pane, including generated feedback
j/k        move selection or scroll focused feedback pane
Up/Down    move selection or scroll focused feedback pane
Enter      expand requirement, play transcript segment, or edit notes
Space      pause/resume current playback
l          link/unlink selected transcript segment to active requirement
a          auto-link evidence to requirements with OpenAI
f          generate feedback from current notes and requirements
n          edit teacher note
Esc        leave note editing
1 / +      mark strong
2 / w      mark weak
3 / m      mark missing
0          mark unseen
e          export verbalstudio-feedback.md
s          export verbalstudio-assessment.json
q          quit
```

## Current Shape

- Requirements are parsed from headings, bullets, and non-empty assignment lines.
- MP3/audio input can automatically create a same-name `.srt` through `scripts/transcribe.py`.
- OpenAI auto-assessment can suggest requirement statuses, notes, and evidence links for review.
- OpenAI feedback generation can turn reviewed notes into the requested feedback format. Grades are validated against the Danish 7-step scale (-3, 00, 02, 4, 7, 10, 12).
- SRT transcript chunks are parsed into timestamped segments.
- Linked evidence is stored by segment index on each requirement.
- Playback is handled inside the TUI process through `rodio`; no external player window is opened.
- Markdown and JSON exports are written in the project directory.

## Direction

The Rust TUI is the fast workflow lab. The core data shape should stay portable:

```text
audio path
assignment path
srt path
requirements[]
segments[]
notes
linked evidence
statuses
```

That same `.assessment.json` can later feed a Svelte app or a backend API.

## GitHub Notes

Local audio, generated transcripts, generated assessment exports, Python caches,
and Rust build artifacts are ignored by `.gitignore`. The checked-in examples are
text-only sample inputs.
