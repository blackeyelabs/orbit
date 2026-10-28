#!/usr/bin/env python3
"""Makes the fixture vault Orbit is measured against
(docs/research/performance.md). The same every time: no randomness.

    scripts/make-fixture-vault.py <empty or new folder> [--notes N]

- N notes (5000 by default) of about 2 KB in 50 folders, each linking to
  three others and tagged;
- Long.md, about 200 KB, and Big.md, about 3 MB (opened as source only);
- Diagrams.md, with 10 Mermaid flowcharts;
- Private/, a folder to protect, with 20 notes.
"""

import argparse
import os
import sys

WORDS = (
    "orbit note link vault paper search index folder diagram mermaid tag "
    "draft review plan idea meeting garden kettle lantern harbour rope "
    "meadow stone river window ledger compass signal anchor"
).split()


def paragraph(seed: int, words: int) -> str:
    return " ".join(WORDS[(seed * 7 + i * 13) % len(WORDS)] for i in range(words))


def note_name(i: int) -> str:
    return f"Folder {i % 50:02}/Note {i:05}.md"


def note(i: int, total: int) -> str:
    links = " ".join(f"[[Note {(i * 31 + k * 97) % total:05}]]" for k in range(1, 4))
    tags = f"#topic/{WORDS[i % len(WORDS)]} #batch{i % 10}"
    body = "\n\n".join(paragraph(i + p, 60) for p in range(5))
    return f"# Note {i:05}\n\n{tags}\n\nSee {links}.\n\n## Details\n\n{body}\n"


def write(root: str, path: str, text: str) -> None:
    full = os.path.join(root, path)
    os.makedirs(os.path.dirname(full) or root, exist_ok=True)
    with open(full, "w", encoding="utf-8") as f:
        f.write(text)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("folder")
    parser.add_argument("--notes", type=int, default=5000)
    args = parser.parse_args()
    root = args.folder
    if os.path.exists(root) and os.listdir(root):
        print(f"{root} isn't empty", file=sys.stderr)
        return 1
    os.makedirs(root, exist_ok=True)
    for i in range(args.notes):
        write(root, note_name(i), note(i, args.notes))
    lines = [f"- {paragraph(i, 12)}" for i in range(2_500)]
    write(root, "Long.md", "# Long\n\n" + "\n".join(lines) + "\n")
    lines = [f"- {i:06} {paragraph(i, 12)}" for i in range(36_000)]
    write(root, "Big.md", "# Big\n\n" + "\n".join(lines) + "\n")
    blocks = "\n\n".join(
        f"## Diagram {d}\n\n```mermaid\nflowchart LR\n"
        + "\n".join(f"  N{d}_{k}[Step {k}] --> N{d}_{k + 1}[Step {k + 1}]" for k in range(8))
        + "\n```"
        for d in range(10)
    )
    write(root, "Diagrams.md", "# Diagrams\n\n" + blocks + "\n")
    for i in range(20):
        write(root, f"Private/Secret {i:02}.md", f"# Secret {i:02}\n\n{paragraph(i, 80)}\n")
    print(f"made {args.notes + 23} notes in {root}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
