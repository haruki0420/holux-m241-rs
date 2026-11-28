#!/usr/bin/env bash
# Run every gate and print one line per gate. Exit 0 only when all pass.
set -uo pipefail

cd "$(dirname "$0")/.." || exit 1

# `m241-app` embeds `ui/dist` through `tauri::generate_context!`, so a clean
# checkout cannot even build the crate until the frontend exists: the proc
# macro panics with "frontendDist ./ui/dist but this path doesn't exist". Build
# it first, once. When pnpm is unavailable or the build fails, the crate is
# excluded from clippy/test and the run says so; the other gates still run, and
# the total is reported as 5/6 plus an explicit skip rather than a fake 6/6.
dist=crates/m241-app/ui/dist/index.html
skip_app=0
skip_reason=""
if [[ ! -f "$dist" ]]; then
  if command -v pnpm >/dev/null 2>&1; then
    printf 'prepare: crates/m241-app/ui/dist is missing; running pnpm install + build\n'
    install_log="${TMPDIR:-/tmp}/m241-check-pnpm-install.log"
    build_log="${TMPDIR:-/tmp}/m241-check-pnpm-build.log"
    if ! pnpm --dir crates/m241-app/ui install --frozen-lockfile >"$install_log" 2>&1; then
      skip_app=1
      skip_reason="ui/dist missing: pnpm install --frozen-lockfile failed ($install_log)"
    elif ! pnpm --dir crates/m241-app/ui build >"$build_log" 2>&1; then
      skip_app=1
      skip_reason="ui/dist missing: pnpm build failed ($build_log)"
    fi
  else
    skip_app=1
    skip_reason="ui/dist missing: pnpm not found (install Node.js and pnpm, then rerun)"
  fi
  if [[ "$skip_app" -eq 0 && -f "$dist" ]]; then
    printf 'prepare: built crates/m241-app/ui/dist\n'
  fi
fi

app_exclude=""
skipped=0
if [[ "$skip_app" -eq 1 ]]; then
  app_exclude="--exclude m241-app"
  skipped=1
  printf '  [SKIP]  m241-app: %s\n' "$skip_reason"
fi

gates=(
  "fmt|cargo fmt --all -- --check"
  "clippy|cargo clippy --workspace --exclude m241-gui --all-targets $app_exclude -- -D warnings"
  "test|cargo test --workspace --exclude m241-gui $app_exclude"
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
passed=$(( ${#gates[@]} - failed - skipped ))
if [[ "$skipped" -eq 1 ]]; then
  printf 'check: %d/%d passed, 1 skipped (m241-app: %s) in %ds\n' \
    "$passed" "${#gates[@]}" "$skip_reason" "$total"
else
  printf 'check: %d/%d passed in %ds\n' "$passed" "${#gates[@]}" "$total"
fi
exit $(( failed > 0 ))
