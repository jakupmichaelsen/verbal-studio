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
    "required": ["requirements"],
    "properties": {
        "requirements": {
            "type": "array",
            "items": {
                "type": "object",
                "additionalProperties": False,
                "required": [
                    "requirement_index",
                    "status",
                    "note",
                    "evidence_segment_indices",
                ],
                "properties": {
                    "requirement_index": {"type": "integer"},
                    "status": {
                        "type": "string",
                        "enum": ["unseen", "strong", "weak", "missing"],
                    },
                    "note": {"type": "string"},
                    "evidence_segment_indices": {
                        "type": "array",
                        "items": {"type": "integer"},
                    },
                },
            },
        }
    },
}


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Map transcript evidence to VerbalStudio requirements with OpenAI."
    )
    parser.add_argument("--input-json", help="assessment payload; defaults to stdin")
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
                    "You prepare oral-presentation assessments for human teacher review. "
                    "Map transcript segments to assignment requirements. Do not grade finally. "
                    "Only use evidence that is actually present in the transcript. "
                    "Use missing when no relevant evidence exists, weak when evidence is vague "
                    "or superficial, strong when evidence clearly fulfils the requirement, and "
                    "unseen only when the requirement cannot be judged from the transcript. "
                    "Return concise teacher-facing notes."
                ),
            },
            {
                "role": "user",
                "content": (
                    "Return JSON that follows the schema. Use segment indices exactly as provided.\n\n"
                    + json.dumps(payload, ensure_ascii=False)
                ),
            },
        ],
        text={
            "format": {
                "type": "json_schema",
                "name": "verbalstudio_auto_assessment",
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
