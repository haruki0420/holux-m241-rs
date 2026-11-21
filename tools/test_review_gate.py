#!/usr/bin/env python3
"""Tests for the pure helpers in ``review_gate.py``.

Run from the repository root:

    python3 -m unittest discover -s tools -p 'test_*.py'

Only the parts that decide something without touching ``gh`` are covered: the
SHA shape, the URL check, the description truncation and the verdict-to-state
mapping. The posting path needs an authenticated ``gh`` and is not exercised
here. Nothing outside the standard library is imported.
"""

from __future__ import annotations

import unittest

import review_gate


class ShaTest(unittest.TestCase):
    def test_a_full_lowercase_sha_is_accepted(self) -> None:
        review_gate.check_sha("cbcd0162ea2835b9b68729cb93f8129529eca330")

    def test_uppercase_hex_is_accepted(self) -> None:
        review_gate.check_sha("CBCD0162EA2835B9B68729CB93F8129529ECA330")

    def test_a_short_sha_is_refused_with_the_length_in_the_message(self) -> None:
        with self.assertRaises(review_gate.GateError) as caught:
            review_gate.check_sha("cbcd016")
        self.assertIn("40-character", str(caught.exception))

    def test_a_sha_of_the_right_length_but_not_hex_is_refused(self) -> None:
        with self.assertRaises(review_gate.GateError):
            review_gate.check_sha("z" * 40)

    def test_a_41_character_sha_is_refused(self) -> None:
        with self.assertRaises(review_gate.GateError):
            review_gate.check_sha("a" * 41)


class UrlTest(unittest.TestCase):
    def test_https_with_a_host_is_accepted(self) -> None:
        review_gate.check_url("https://github.com/o/r/pull/2#issuecomment-1")

    def test_http_with_a_host_is_accepted(self) -> None:
        review_gate.check_url("http://example.test/review")

    def test_a_non_http_scheme_is_refused(self) -> None:
        for url in ("ftp://example.test/x", "javascript:alert(1)", "file:///etc/passwd"):
            with self.subTest(url=url):
                with self.assertRaises(review_gate.GateError):
                    review_gate.check_url(url)

    def test_a_url_without_a_host_is_refused(self) -> None:
        for url in ("https://", "http:///path"):
            with self.subTest(url=url):
                with self.assertRaises(review_gate.GateError):
                    review_gate.check_url(url)

    def test_an_empty_url_is_refused(self) -> None:
        with self.assertRaises(review_gate.GateError):
            review_gate.check_url("")


class DescribeTest(unittest.TestCase):
    def test_a_short_description_is_left_alone(self) -> None:
        text = review_gate.describe("2", "mergeable", "stage1:ses_x")
        self.assertEqual(text, "Tier 2 independent review: mergeable (stage1:ses_x)")

    def test_exactly_the_limit_is_not_truncated(self) -> None:
        prefix = "Tier 3 independent review: blocked ("
        reviewer = "x" * (review_gate.MAX_DESCRIPTION - len(prefix) - 1)
        text = review_gate.describe("3", "blocked", reviewer)
        self.assertEqual(len(text), review_gate.MAX_DESCRIPTION)
        self.assertFalse(text.endswith("…"))

    def test_over_the_limit_is_truncated_to_the_limit_with_an_ellipsis(self) -> None:
        text = review_gate.describe("3", "blocked", "x" * 200)
        self.assertEqual(len(text), review_gate.MAX_DESCRIPTION)
        self.assertTrue(text.endswith("…"))


class StateTest(unittest.TestCase):
    def test_mergeable_maps_to_success(self) -> None:
        self.assertEqual(review_gate.state_for("mergeable"), "success")

    def test_blocked_maps_to_failure(self) -> None:
        self.assertEqual(review_gate.state_for("blocked"), "failure")


class HeadTest(unittest.TestCase):
    HEAD = "cbcd0162ea2835b9b68729cb93f8129529eca330"

    def test_a_matching_head_passes(self) -> None:
        review_gate.check_head("open", self.HEAD, self.HEAD, 2)

    def test_a_moved_head_is_refused_and_names_the_new_sha(self) -> None:
        new = "8" * 40
        with self.assertRaises(review_gate.GateError) as caught:
            review_gate.check_head("open", new, self.HEAD, 2)
        message = str(caught.exception)
        self.assertIn(new, message)
        self.assertIn("rerun with that SHA", message)

    def test_a_closed_pr_is_refused(self) -> None:
        with self.assertRaises(review_gate.GateError):
            review_gate.check_head("closed", self.HEAD, self.HEAD, 2)


class CommandTest(unittest.TestCase):
    def test_the_status_command_carries_the_four_required_fields(self) -> None:
        command = review_gate.build_status_command(
            "o/r", "a" * 40, "success", "d", "https://example.test/x"
        )
        joined = " ".join(command)
        self.assertIn(f"repos/o/r/statuses/{'a' * 40}", joined)
        for field in ("state=success", f"context={review_gate.CONTEXT}", "description=d"):
            self.assertIn(field, joined)
        self.assertIn("target_url=https://example.test/x", joined)


if __name__ == "__main__":
    unittest.main()
