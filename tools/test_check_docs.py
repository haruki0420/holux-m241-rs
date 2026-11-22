#!/usr/bin/env python3
"""Tests for the documentation-gate walk in ``check_docs.py``.

Run from the repository root:

    python3 -m unittest discover -s tools -p 'test_*.py'

The gate resolves its scan root from ``check_docs.REPO``, so each walk test
points that at a temporary tree. What they pin is the exclusion of the agent
work trees: ``.agents/`` holds independent checkouts on other branches, and a
page there may name a path its own branch has not received yet. Scanning it from
the main checkout therefore reported a valid page as a broken link.

The two end-to-end tests are a pair on purpose: the same broken reference is
ignored under ``.agents/`` and failed under ``docs/``, so the exclusion cannot
pass by the gate having stopped looking at its own documentation.
"""

from __future__ import annotations

import io
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest import mock

import check_docs


class SkipDirsTest(unittest.TestCase):
    def test_agent_work_trees_are_skipped(self) -> None:
        self.assertIn(".agents", check_docs.SKIP_DIRS)


class WalkTest(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.root = Path(self._tmp.name)

    def write(self, relative: str, body: str = "# page\n") -> Path:
        path = self.root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(body, encoding="utf-8")
        return path

    def gate(self) -> int:
        with mock.patch.object(check_docs, "REPO", self.root), redirect_stdout(
            io.StringIO()
        ):
            return check_docs.main()

    def test_a_page_in_an_agent_work_tree_is_not_scanned(self) -> None:
        self.write("docs/page.md")
        self.write(".agents/worktrees/feat/docs/page.md")
        with mock.patch.object(check_docs, "REPO", self.root):
            found = [
                path.relative_to(self.root).as_posix()
                for path in check_docs.markdown_files()
            ]
        self.assertEqual(found, ["docs/page.md"])

    def test_a_path_that_exists_only_in_an_agent_work_tree_does_not_resolve(
        self,
    ) -> None:
        document = self.write("docs/page.md")
        self.write(".agents/worktrees/feat/tools/work-tree-only.py")
        with mock.patch.object(check_docs, "REPO", self.root):
            self.assertFalse(check_docs.resolves(document, "work-tree-only.py"))

    def test_a_path_outside_the_agent_work_trees_still_resolves(self) -> None:
        document = self.write("docs/page.md")
        self.write("tools/checkout-only.py")
        with mock.patch.object(check_docs, "REPO", self.root):
            self.assertTrue(check_docs.resolves(document, "checkout-only.py"))

    def test_a_broken_reference_in_an_agent_work_tree_is_ignored(self) -> None:
        self.write(".agents/worktrees/feat/docs/stale.md", "[x](missing.md)\n")
        self.assertEqual(self.gate(), 0)

    def test_a_broken_reference_outside_the_work_trees_still_fails(self) -> None:
        self.write("docs/stale.md", "[x](missing.md)\n")
        self.assertEqual(self.gate(), 1)


if __name__ == "__main__":
    unittest.main()
