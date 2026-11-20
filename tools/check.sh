#!/usr/bin/env bash
# Run every gate and print one line per gate. Exit 0 only when all pass.
set -uo pipefail

cd "$(dirname "$0")/.." || exit 1

gates=(
  "fmt|cargo fmt --all -- --check"
  "clippy|cargo clippy --all-targets -- -D warnings"
  "test|cargo test --workspace"
  "docs|python3 tools/check_docs.py"
  "design|python3 tools/check_design.py"
  "safety|python3 tools/check_safety.py"
)

failed=0
started=$(date +%s)
printf 'check: %d gates in %s\n' "${#gates[@]}" "$PWD"

for gate in "${gates[@]}"; do
  name=${gate%%|*}
  command=${gate#*|}
  t0=$(date +%s%N)
  if output=$(eval "$command" 2>&1); then
    status=PASS
  else
    status=FAIL
    failed=$((failed + 1))
    printf '%s\n' "$output" | tail -20
  fi
  t1=$(date +%s%N)
  printf '  [%s] %-7s %6.1fs\n' "$status" "$name" "$(echo "scale=1; ($t1-$t0)/1000000000" | bc)"
done

total=$(( $(date +%s) - started ))
printf 'check: %d/%d passed in %ds\n' "$(( ${#gates[@]} - failed ))" "${#gates[@]}" "$total"
exit $(( failed > 0 ))
