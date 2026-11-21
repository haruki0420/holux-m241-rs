#!/usr/bin/env python3
"""Attach the independent-review status to the exact PR head that was reviewed.

The status is the machine-enforced half of the review gate: branch protection
requires ``review-gate`` on the current head, so a push invalidates the old
status and a fresh review is required. The script cannot prove that a review
happened; it records the reviewer's attestation and the head SHA they read.

Run from the repository root, after the independent review is written:

    python3 tools/review_gate.py <head-sha> --pr 12 --verdict mergeable \
        --tier 2 --reviewer "stage3:ses_..." \
        --review-url "https://github.com/.../pull/12#issuecomment-..."

Only the reviewer session runs this. The author session must not.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys

CONTEXT = "review-gate"
MAX_DESCRIPTION = 140


def run_gh(args: list[str]) -> str:
    """Run ``gh`` and return stdout, failing loudly with its stderr."""
    result = subprocess.run(
        ["gh", *args],
        check=False,
        text=True,
        capture_output=True,
    )
    if result.returncode != 0:
        sys.exit(f"gh {' '.join(args)} failed:\n{result.stderr.strip()}")
    return result.stdout


def gh_json(args: list[str]) -> dict:
    output = run_gh(args).strip()
    if not output:
        sys.exit(f"gh {' '.join(args)} returned no JSON")
    try:
        return json.loads(output)
    except json.JSONDecodeError as error:
        sys.exit(f"gh {' '.join(args)} returned invalid JSON: {error}")


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Post the review-gate status for a reviewed PR head.",
    )
    parser.add_argument("sha", help="the exact head SHA that was reviewed")
    parser.add_argument("--pr", type=int, required=True, help="pull request number")
    parser.add_argument(
        "--verdict",
        choices=("mergeable", "blocked"),
        required=True,
        help="the reviewer's verdict for this head",
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
        help="independent session or agent name (not the author)",
    )
    parser.add_argument(
        "--review-url",
        required=True,
        help="link to the recorded review (PR comment or review-log entry)",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="print the status command without posting it",
    )
    args = parser.parse_args()

    if not args.review_url.startswith(("http://", "https://")):
        sys.exit("--review-url must be an http(s) URL")

    repo = gh_json(["repo", "view", "--json", "nameWithOwner"])["nameWithOwner"]
    pr = gh_json(["api", f"repos/{repo}/pulls/{args.pr}"])

    if pr.get("state") != "open":
        sys.exit(f"PR #{args.pr} is {pr.get('state')}, not open")
    head = pr.get("head", {}).get("sha", "")
    if head != args.sha:
        sys.exit(
            f"refusing: PR #{args.pr} head is {head or '<unknown>'}, "
            f"not the reviewed {args.sha}. Review the new head first."
        )

    state = "success" if args.verdict == "mergeable" else "failure"
    description = (
        f"Tier {args.tier} independent review: {args.verdict} ({args.reviewer})"
    )
    if len(description) > MAX_DESCRIPTION:
        description = description[: MAX_DESCRIPTION - 1] + "…"

    author = pr.get("user", {}).get("login", "")
    if author and author in args.reviewer:
        print(
            f"warning: the PR author is {author}; the reviewer label contains the "
            "same account. The gate is procedural unless a second account or app "
            "posts the status.",
            file=sys.stderr,
        )

    command = [
        "api",
        "-X",
        "POST",
        f"repos/{repo}/statuses/{args.sha}",
        "-f",
        f"state={state}",
        "-f",
        f"context={CONTEXT}",
        "-f",
        f"description={description}",
        "-f",
        f"target_url={args.review_url}",
    ]
    if args.dry_run:
        print("gh " + " ".join(command))
        return 0

    run_gh(command)
    print(
        f"review-gate: {state} on {args.sha} "
        f"(PR #{args.pr}, Tier {args.tier}, {args.reviewer})"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
