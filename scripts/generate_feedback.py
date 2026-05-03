#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import sys

SCRIPT_PATH = Path(__file__).resolve()
SCRIPTS_ROOT = SCRIPT_PATH.parents[2]
PYTHON_VENV = SCRIPTS_ROOT / "Python" / ".venv"
PROJECT_PYTHON = PYTHON_VENV / "bin" / "python"

if PROJECT_PYTHON.exists() and Path(sys.prefix).resolve() != PYTHON_VENV.resolve():
    os.execv(str(PROJECT_PYTHON), [str(PROJECT_PYTHON), str(SCRIPT_PATH), *sys.argv[1:]])

try:
    from openai import OpenAI
except ImportError as exc:
    raise SystemExit(
        "Could not import OpenAI. Run from the Python venv or install a current openai package."
    ) from exc


SCHEMA = {
    "type": "object",
    "additionalProperties": False,
    "required": ["feedback_markdown"],
    "properties": {
        "feedback_markdown": {
            "type": "string",
            "description": "Markdown feedback following the assignment output instructions.",
        }
    },
}


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Generate spoken feedback from VerbalStudio assessment notes."
    )
    parser.add_argument("--input-json", help="feedback payload; defaults to stdin")
    parser.add_argument("--model", default=os.getenv("OPENAI_ASSESSMENT_MODEL", "gpt-4.1-mini"))
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    api_key = os.getenv("OPENAI_API_KEY")
    if not api_key:
        print("OPENAI_API_KEY is not set.", file=sys.stderr)
        return 1

    input_json = args.input_json if args.input_json is not None else sys.stdin.read()
    payload = json.loads(input_json)
    client = OpenAI(api_key=api_key)

    response = client.responses.create(
        model=args.model,
        input=[
            {
                "role": "system",
                "content": (
                    "You generate teacher feedback for an oral presentation. "
                    "Follow the assignment instructions exactly, especially the requested output format. "
                    "Use the reviewed assessment notes and linked evidence as your source of truth. "
                    "Do not invent theme texts, episode references, or strengths not supported by the notes. "
                    "The spoken feedback section must be natural Danish that a teacher could say aloud. "
                    "Be supportive, honest, concise, and concrete."
                ),
            },
            {
                "role": "user",
                "content": (
                    "Return JSON with one field: feedback_markdown. "
                    "The markdown must include the standard feedback sections and a spoken Danish feedback section.\n\n"
                    "FULL ASSIGNMENT / CUSTOM GPT INSTRUCTIONS:\n"
                    f"{payload.get('assignment_instructions', '')}\n\n"
                    "REVIEWED ASSESSMENT NOTES WITH EVIDENCE:\n"
                    f"{payload.get('assessment_notes_markdown', '')}"
                ),
            },
        ],
        text={
            "format": {
                "type": "json_schema",
                "name": "verbalstudio_feedback",
                "strict": True,
                "schema": SCHEMA,
            }
        },
    )

    text = getattr(response, "output_text", "")
    if not text:
        for item in getattr(response, "output", []):
            for content in getattr(item, "content", []):
                if getattr(content, "type", "") == "output_text":
                    text += getattr(content, "text", "")

    parsed = json.loads(text)
    print(json.dumps(parsed, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
