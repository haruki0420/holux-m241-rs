#!/usr/bin/env python3
"""Attach the independent-review status to the exact PR head that was reviewed.

The status is the machine-enforced half of the review gate: branch protection
requires ``review-gate`` on the current head, so a push invalidates the old
status and a fresh review is required. The script cannot prove that a review
happened; it records the reviewer's attestation and the head SHA they read.

Run from the repository root, after the independent review is written:

    python3 tools/review_gate.py <40-character head-sha> --pr 12 \
        --verdict mergeable --tier 2 --reviewer "stage3:ses_..." \
        --review-url "https://github.com/.../pull/12#issuecomment-..."

``--review-url`` is both the reviewer's record (a PR comment or an entry in
``docs/dev/10-review-log.md``) and the ``target_url`` of the status, so the
status card links to the evidence. For ``--verdict blocked`` it is also where
the reason is written.

``--dry-run`` still calls ``gh``: it reads the PR head so the SHA check runs,
prints the status command (never posts it), and exits. It needs a working,
authenticated ``gh`` and network access; it does not work offline.

Only the reviewer session runs this. The author session must not.
"""

from __future__ import annotations

import argparse
import json
import shlex
import subprocess
import sys
from urllib.parse import urlparse

#: The status context branch protection requires. It is an external contract:
#: `docs/dev/09-review-protocol.md` and the branch protection rule name the same
#: string, so it lives here once.
CONTEXT = "review-gate"

#: GitHub truncates a commit status description at 140 characters. Counted in
#: Python characters (code points): the description this script builds is ASCII
#: apart from the trailing ellipsis, so code points, UTF-16 units and bytes agree
#: for every value it produces.
MAX_DESCRIPTION = 140

#: A commit status is addressed by the full object id. Abbreviated SHAs are
#: rejected rather than resolved: resolution is an API round trip and leaves
#: room for two commits starting with the same prefix.
FULL_SHA_LENGTH = 40


class GateError(Exception):
    """A refusal or a `gh` failure, reported to the user as a single message."""


def run_gh(args: list[str]) -> str:
    """Run ``gh`` and return stdout, raising ``GateError`` with its stderr."""
    result = subprocess.run(
        ["gh", *args],
        check=False,
        text=True,
        capture_output=True,
    )
    if result.returncode != 0:
        raise GateError(f"gh {' '.join(args)} failed:\n{result.stderr.strip()}")
    return result.stdout


def gh_json(args: list[str]) -> dict:
    """Run ``gh`` and parse its stdout as a JSON object."""
    output = run_gh(args).strip()
    if not output:
        raise GateError(f"gh {' '.join(args)} returned no JSON")
    try:
        parsed = json.loads(output)
    except json.JSONDecodeError as error:
        raise GateError(f"gh {' '.join(args)} returned invalid JSON: {error}") from error
    if not isinstance(parsed, dict):
        raise GateError(f"gh {' '.join(args)} returned JSON that is not an object")
    return parsed


def check_sha(sha: str) -> None:
    """Refuse anything but a full 40-character hexadecimal SHA."""
    if len(sha) != FULL_SHA_LENGTH or any(ch not in "0123456789abcdefABCDEF" for ch in sha):
        raise GateError(
            f"refusing: {sha!r} is not a full 40-character SHA. Pass the whole "
            "head SHA: the status is attached to that commit, and a short SHA "
            "cannot be told apart from another commit with the same prefix."
        )


def check_url(url: str) -> None:
    """Refuse anything that is not an absolute http(s) URL with a host."""
    parsed = urlparse(url)
    if parsed.scheme not in ("http", "https") or not parsed.netloc:
        raise GateError(
            f"--review-url must be an absolute http(s) URL with a host, got {url!r}"
        )


def fetch_pr(repo: str, number: int) -> dict:
    """The PR object from `gh api repos/{repo}/pulls/{n}`.

    Required keys: ``state`` (must be ``open``), ``head.sha`` (compared with the
    reviewed SHA) and ``user.login`` (the author, for the warning below). A
    missing key is a contract change in the GitHub API; it surfaces as an
    unknown state or an empty head rather than an exception.
    """
    return gh_json(["api", f"repos/{repo}/pulls/{number}"])


def check_head(pr_state: object, head: str, sha: str, number: int) -> None:
    """Refuse a closed PR or a head that is not the reviewed SHA."""
    if pr_state != "open":
        raise GateError(f"PR #{number} is {pr_state}, not open")
    if head != sha:
        shown = head or "<unknown>"
        raise GateError(
            f"refusing: PR #{number} head is {shown}, not the reviewed {sha}. "
            f"Review {shown} and rerun with that SHA."
        )


def describe(tier: str, verdict: str, reviewer: str) -> str:
    """The status description, truncated to GitHub's limit with an ellipsis."""
    description = f"Tier {tier} independent review: {verdict} ({reviewer})"
    if len(description) > MAX_DESCRIPTION:
        description = description[: MAX_DESCRIPTION - 1] + "…"
    return description


def state_for(verdict: str) -> str:
    """Map the verdict onto the status state branch protection understands."""
    return "success" if verdict == "mergeable" else "failure"


def warn_if_author(author: str, reviewer: str) -> None:
    """Warn when the reviewer label names the PR author.

    A label such as ``stage3:ses_...`` does not contain the account name, so
    this cannot detect self-review; it only catches the obvious case.
    """
    if not author:
        print(
            "warning: the PR author could not be read from the API; the "
            "self-review check did not run.",
            file=sys.stderr,
        )
    elif author in reviewer:
        print(
            f"warning: the PR author is {author}; the reviewer label contains the "
            "same account. The gate is procedural unless a second account or app "
            "posts the status.",
            file=sys.stderr,
        )


def build_status_command(
    repo: str, sha: str, state: str, description: str, review_url: str
) -> list[str]:
    """The `gh api` argument list that posts the status."""
    return [
        "api",
        "-X",
        "POST",
        f"repos/{repo}/statuses/{sha}",
        "-f",
        f"state={state}",
        "-f",
        f"context={CONTEXT}",
        "-f",
        f"description={description}",
        "-f",
        f"target_url={review_url}",
    ]


def post_status(command: list[str]) -> None:
    """POST the status and discard the response body."""
    run_gh(command)


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Post the review-gate status for a reviewed PR head.",
    )
    parser.add_argument(
        "sha",
        help="the full 40-character head SHA that was reviewed",
    )
    parser.add_argument("--pr", type=int, required=True, help="pull request number")
    parser.add_argument(
        "--verdict",
        choices=("mergeable", "blocked"),
        required=True,
        help="the reviewer's verdict for this head (blocked posts failure)",
    )
    parser.add_argument(
        "--tier",
        choices=("1", "2", "3"),
        required=True,
        help="review depth: 1 = diff/tests, 2 = two reviews, 3 = safety-critical",
    )
    parser.add_argument(
        "--reviewer",
        required=True,
        help='independent session label, e.g. "stage3:ses_..." (not the author)',
    )
    parser.add_argument(
        "--review-url",
        required=True,
        help=(
            "http(s) URL of the recorded review (PR comment or review-log entry). "
            "It is also the status target_url and, for blocked, where the reason is"
        ),
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help=(
            "check the PR head with gh and print the status command, but do not "
            "post it (still needs an authenticated gh and network access)"
        ),
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    try:
        check_sha(args.sha)
        check_url(args.review_url)

        repo = gh_json(["repo", "view", "--json", "nameWithOwner"])["nameWithOwner"]
        pr = fetch_pr(repo, args.pr)
        head = pr.get("head", {}).get("sha", "")
        check_head(pr.get("state"), head, args.sha, args.pr)

        state = state_for(args.verdict)
        description = describe(args.tier, args.verdict, args.reviewer)
        warn_if_author(pr.get("user", {}).get("login", ""), args.reviewer)

        command = build_status_command(
            repo, args.sha, state, description, args.review_url
        )
        if args.dry_run:
            # Quoted so the line can be pasted back into a shell unchanged.
            print(shlex.join(["gh", *command]))
            return 0
        post_status(command)
    except GateError as error:
        sys.exit(str(error))

    print(
        f"{CONTEXT}: {state} on {args.sha} "
        f"(PR #{args.pr}, Tier {args.tier}, {args.reviewer})"
    )
    print(f"target_url: {args.review_url}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
