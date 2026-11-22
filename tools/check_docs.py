#!/usr/bin/env python3
"""Fail if a relative Markdown link or backticked repository path is broken.

Run from the repository root:

    python3 tools/check_docs.py

Two kinds of reference are checked:

1. **Markdown links** -- ``[label](target)``. Absolute URLs, mailto and anchors
   are skipped.
2. **Inline code paths** -- `` `docs/reference/01-da-protocol.md` ``. These are
   not links, so a plain link checker misses them, and they are exactly how a
   documentation reorganisation rots silently.

Fenced code blocks are skipped for links (they hold shell commands, not
references); inline code is inspected only when it looks like a path into this
repository.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
LINK_RE = re.compile(r"\[[^\]]*\]\(([^)\s]+)(?:\s+\"[^\"]*\")?\)")
FENCE_RE = re.compile(r"^\s*(```|~~~)")
INLINE_CODE_RE = re.compile(r"`([^`\n]+)`")
SKIP_PREFIXES = ("http://", "https://", "mailto:", "#", "data:")
#: Directories that are not part of this checkout's documentation. `.worktrees`
#: holds the parallel work trees and `.agents` the agent work trees, each a full
#: copy of the repository on its own branch: scanning one would check every page
#: once per tree and report paths that are only missing because that branch has
#: not received them yet.
SKIP_DIRS = {
    ".git",
    "target",
    ".cargo-home",
    "node_modules",
    ".worktrees",
    ".agents",
    "vendor",
    "_scratch",
    ".inbox",
}

#: Directories whose contents are referenced by path in the docs.
PATH_ROOTS = ("docs/", "crates/", "tools/", ".github/")
#: Paths the user creates; they are gitignored and must not be required to exist.
USER_ROOTS = ("backup/", "vendor/", "holux/", "dist/", "captures/")
#: Extensions that are always user data or build output.
USER_SUFFIXES = (".bin", ".gpx", ".kml", ".csv", ".log", ".jpg", ".jpeg", ".png", ".zip")
#: Bare filenames that are referenced by name and must exist somewhere.
PATH_SUFFIXES = (".md", ".rs", ".toml", ".json", ".yml", ".yaml", ".sh", ".py")
#: Files whose inline paths are historical records, not live references.
INLINE_PATH_SKIP_FILES = {"CHANGELOG.md"}
#: Put this in a line to opt out of the inline-path check. Used by planning
#: documents that name files which do not exist yet; the marker makes the
#: forward reference deliberate instead of a silent stale path.
IGNORE_MARKER = "path-check: ignore"


def markdown_files() -> list[Path]:
    out = []
    for path in REPO.rglob("*.md"):
        if any(part in SKIP_DIRS for part in path.relative_to(REPO).parts):
            continue
        out.append(path)
    return sorted(out)


def links_in(path: Path) -> list[tuple[int, str]]:
    found: list[tuple[int, str]] = []
    in_fence = False
    for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), start=1):
        if FENCE_RE.match(line):
            in_fence = not in_fence
            continue
        if in_fence:
            continue
        for match in LINK_RE.finditer(INLINE_CODE_RE.sub("", line)):
            target = match.group(1)
            if target.startswith(SKIP_PREFIXES):
                continue
            found.append((number, target))
    return found


def code_paths_in(path: Path) -> list[tuple[int, str]]:
    if path.name in INLINE_PATH_SKIP_FILES:
        return []
    found: list[tuple[int, str]] = []
    for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), start=1):
        if IGNORE_MARKER in line:
            continue
        for match in INLINE_CODE_RE.finditer(line):
            token = match.group(1).strip()
            if not token or token.startswith(SKIP_PREFIXES):
                continue
            if any(ch in token for ch in "*?<>|$ "):
                continue
            if token.startswith(USER_ROOTS) or token.lower().endswith(USER_SUFFIXES):
                continue
            if token.startswith(PATH_ROOTS):
                found.append((number, token))
            elif token.count("/") == 0 and token.endswith(PATH_SUFFIXES):
                found.append((number, token))
    return found


def resolves(document: Path, token: str) -> bool:
    cleaned = token.split("#", 1)[0].split(":", 1)[0].strip()
    if not cleaned:
        return True
    if cleaned.startswith(PATH_ROOTS) or "/" in cleaned:
        return (document.parent / cleaned).resolve().exists() or (REPO / cleaned).exists()
    return any(
        not any(part in SKIP_DIRS for part in match.relative_to(REPO).parts)
        for match in REPO.rglob(cleaned)
    )


def main() -> int:
    problems: list[str] = []
    link_count = 0
    path_count = 0
    documents = markdown_files()

    for document in documents:
        rel_doc = document.relative_to(REPO)
        for number, target in links_in(document):
            cleaned = target.split("#", 1)[0]
            if not cleaned:
                continue
            link_count += 1
            if not (document.parent / cleaned).resolve().exists():
                problems.append(f"{rel_doc}:{number}: missing link target {target}")
        for number, token in code_paths_in(document):
            path_count += 1
            if not resolves(document, token):
                problems.append(f"{rel_doc}:{number}: missing path `{token}`")

    print(
        f"checked {link_count} relative link(s) and {path_count} inline path(s) "
        f"in {len(documents)} markdown file(s)"
    )
    for problem in problems:
        print(f"  [!] {problem}")
    if problems:
        print(f"{len(problems)} broken reference(s)")
        return 1
    print("all links resolve")
    return 0


if __name__ == "__main__":
    sys.exit(main())
