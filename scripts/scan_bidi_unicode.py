#!/usr/bin/env python3
from __future__ import annotations

import bisect
import sys
from pathlib import Path

ALLOWED_EXTS = {
    ".rs",
    ".ps1",
    ".ts",
    ".tsx",
    ".js",
    ".jsx",
    ".json",
    ".toml",
    ".yml",
    ".yaml",
    ".md",
}

BIDI_CODEPOINTS = set(range(0x202A, 0x202F)) | set(range(0x2066, 0x206A))
SKIP_DIRS = {
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    ".venv",
    "venv",
    "__pycache__",
}


def iter_target_files(root: Path):
    for path in root.rglob("*"):
        if not path.is_file():
            continue
        if any(part in SKIP_DIRS for part in path.parts):
            continue
        if path.suffix.lower() not in ALLOWED_EXTS:
            continue
        yield path


def line_starts(text: str) -> list[int]:
    starts = [0]
    for idx, ch in enumerate(text):
        if ch == "\n":
            starts.append(idx + 1)
    return starts


def to_line_col(starts: list[int], pos: int) -> tuple[int, int]:
    line_idx = bisect.bisect_right(starts, pos) - 1
    line = line_idx + 1
    col = pos - starts[line_idx] + 1
    return line, col


def make_context(text: str, pos: int, radius: int = 20) -> str:
    left = max(0, pos - radius)
    right = min(len(text), pos + radius + 1)
    snippet = text[left:right]
    return snippet.replace("\r", "\\r").replace("\n", "\\n")


def scan_file(path: Path) -> list[tuple[int, int, int, str]]:
    text = path.read_text(encoding="utf-8", errors="replace")
    starts = line_starts(text)
    hits: list[tuple[int, int, int, str]] = []
    for i, ch in enumerate(text):
        cp = ord(ch)
        if cp in BIDI_CODEPOINTS:
            line, col = to_line_col(starts, i)
            hits.append((line, col, cp, make_context(text, i)))
    return hits


def main() -> int:
    repo_root = Path(__file__).resolve().parents[1]
    found = 0

    for path in iter_target_files(repo_root):
        hits = scan_file(path)
        if not hits:
            continue
        rel = path.relative_to(repo_root).as_posix()
        for line, col, cp, ctx in hits:
            print(f"{rel}:{line}:{col} U+{cp:04X} context=\"{ctx}\"")
            found += 1

    if found:
        print(f"Found {found} forbidden bidi/control character(s).", file=sys.stderr)
        return 1

    print("No forbidden bidi/control characters found.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
