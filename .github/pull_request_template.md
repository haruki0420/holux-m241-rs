## Summary

<!-- What changed, and why. -->

## Review gate

<!-- The gate is machine-enforced by the `review-gate` status on the head SHA.
     Fill this in before asking for merge. -->

- Tier: <!-- 1 (1–100 non-test lines, docs/tests only), 2 (101–300, one module), 3 (safety-critical, 301+, or several modules) -->
- Non-test lines changed: <!-- added+deleted from `git diff --numstat`, excluding tests/ and *_test.rs; inline tests count. >300 needs a split rationale -->
- If >300, why not split: <!-- ... -->
- Stage 1 specification review: <!-- link; required in every tier -->
- Stage 1 readability review: <!-- link, or "n/a" for Tier 1 -->
- Stage 2 fix: <!-- commit -->
- Stage 3 independent verification: <!-- link + 合格/不合格 (the verdict vocabulary of docs/dev/09-review-protocol.md), or "n/a" for Tier 1 unless behaviour changed -->
- Mutation tests: <!-- Tier 2: 3–5, Tier 3: 10+; "n/a" for Tier 1 -->
- Reviewed head SHA: <!-- the full 40-character SHA the reviewer read -->
- `review-gate` status: <!-- link to the status or the review comment; required in every tier -->

## Checks

- [ ] `./tools/check.sh` 6/6
- [ ] All required CI checks pass
- [ ] `review-gate` is on the current head SHA (it is per-commit; a new push needs a new review)
- [ ] No safety gate was weakened, and no unverified hardware claim was promoted

## Notes for the reviewer

<!-- Where to start, what is risky, what was not verified on hardware. -->
