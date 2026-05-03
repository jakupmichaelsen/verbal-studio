#!/usr/bin/env python3
from __future__ import annotations

import argparse
from contextlib import contextmanager
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

try:
    from openai import OpenAI
except ImportError as exc:
    raise SystemExit(
        "Could not import OpenAI. Install the Python SDK first:\n"
        "  python3 -m pip install --upgrade openai"
    ) from exc


FORMAT_SUFFIXES = {
    "text": ".md",
    "srt": ".srt",
    "vtt": ".vtt",
    "json": ".json",
    "verbose_json": ".json",
}

TARGET_UPLOAD_BYTES = 24 * 1024 * 1024
TRANSCODE_BITRATES = ("64k", "48k", "32k", "24k", "16k")


def run_ffmpeg(src: Path, dst: Path, bitrate: str) -> None:
    command = [
        "ffmpeg",
        "-hide_banner",
        "-loglevel",
        "error",
        "-y",
        "-i",
        str(src),
        "-vn",
        "-ac",
        "1",
        "-b:a",
        bitrate,
        str(dst),
    ]
    subprocess.run(command, check=True)


@contextmanager
def uploadable_audio(src: Path):
    if src.stat().st_size <= TARGET_UPLOAD_BYTES:
        yield src
        return

    if not shutil.which("ffmpeg"):
        raise RuntimeError(
            f"{src.name} is {src.stat().st_size:,} bytes, above the safe upload "
            "limit, and ffmpeg is not available to make a smaller temporary copy."
        )

    with tempfile.TemporaryDirectory(prefix="verbalstudio-upload-") as tmpdir:
        tmp = Path(tmpdir)
        last_size = src.stat().st_size
        for bitrate in TRANSCODE_BITRATES:
            candidate = tmp / f"{src.stem}-{bitrate}.mp3"
            run_ffmpeg(src, candidate, bitrate)
            last_size = candidate.stat().st_size
            if last_size <= TARGET_UPLOAD_BYTES:
                print(
                    f"Compressed temporary upload: {candidate.name} "
                    f"({last_size:,} bytes)",
                    file=sys.stderr,
                )
                yield candidate
                return

        raise RuntimeError(
            f"Could not compress {src.name} below {TARGET_UPLOAD_BYTES:,} bytes; "
            f"smallest temporary file was {last_size:,} bytes."
        )


def transcribe_to_file(
    file_path: str,
    api_key: str | None,
    *,
    model: str = "whisper-1",
    response_format: str = "srt",
    output_path: str | None = None,
    language: str | None = None,
    prompt: str | None = None,
) -> Path:
    if not api_key:
        raise ValueError("OPENAI_API_KEY is not set.")

    src = Path(file_path).expanduser().resolve()
    if not src.exists():
        raise FileNotFoundError(f"File not found: {src}")

    client = OpenAI(api_key=api_key)
    request = {
        "model": model,
        "file": None,
        "response_format": response_format,
    }
    if language:
        request["language"] = language
    if prompt:
        request["prompt"] = prompt

    with uploadable_audio(src) as upload_path:
        with upload_path.open("rb") as audio_file:
            request["file"] = audio_file
            transcription = client.audio.transcriptions.create(**request)

    if isinstance(transcription, str):
        text = transcription
    elif response_format in {"json", "verbose_json"}:
        if hasattr(transcription, "model_dump_json"):
            text = transcription.model_dump_json(indent=2)
        else:
            text = json.dumps(transcription, indent=2)
    else:
        text = transcription.text

    out_path = (
        Path(output_path).expanduser().resolve()
        if output_path
        else src.with_suffix(FORMAT_SUFFIXES[response_format])
    )
    out_path.write_text(text, encoding="utf-8")
    print(f"Saved: {out_path}")
    return out_path


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Transcribe audio for VerbalStudio.")
    parser.add_argument("audiofile", help="audio file to transcribe")
    parser.add_argument(
        "-m",
        "--model",
        default="whisper-1",
        help="transcription model to use (default: whisper-1)",
    )
    parser.add_argument(
        "-f",
        "--format",
        choices=sorted(FORMAT_SUFFIXES),
        default="srt",
        help="output format (default: srt)",
    )
    parser.add_argument("-o", "--output", help="output path")
    parser.add_argument("-l", "--language", help="optional input language code")
    parser.add_argument("-p", "--prompt", help="optional transcription prompt")
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        transcribe_to_file(
            args.audiofile,
            os.getenv("OPENAI_API_KEY"),
            model=args.model,
            response_format=args.format,
            output_path=args.output,
            language=args.language,
            prompt=args.prompt,
        )
    except Exception as exc:
        print(f"Error: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
