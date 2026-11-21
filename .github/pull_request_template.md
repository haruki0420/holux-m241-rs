## Summary

<!-- What changed, and why. -->

## Review gate

<!-- The gate is machine-enforced by the `review-gate` status on the head SHA.
     Fill this in before asking for merge. -->

- Tier: <!-- 1 (diff/tests), 2 (two reviews), 3 (safety-critical) -->
- Non-test lines changed: <!-- >300 needs a split rationale -->
- If >300, why not split: <!-- ... -->
- Stage 1 specification review: <!-- link -->
- Stage 1 readability review: <!-- link, or "n/a" for Tier 1 -->
- Stage 2 fix: <!-- commit -->
- Stage 3 independent verification: <!-- link + 合格/不合格 -->
- Reviewed head SHA: <!-- the SHA the reviewer read -->
- `review-gate` status: <!-- link to the status or the review comment -->

## Checks

- [ ] `./tools/check.sh` 6/6
- [ ] All required CI checks pass
- [ ] `review-gate` is on the current head SHA (it is per-commit; a new push needs a new review)
- [ ] No safety gate was weakened, and no unverified hardware claim was promoted

## Notes for the reviewer

<!-- Where to start, what is risky, what was not verified on hardware. -->
