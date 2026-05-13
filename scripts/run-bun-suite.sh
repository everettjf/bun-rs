#!/usr/bin/env bash
# Run a subset of Bun's official test suite against bun-rs and write a
# per-file summary CSV. Used to track API-compatibility progress.
#
# Usage:
#   scripts/run-bun-suite.sh <test-glob-or-dir> [timeout-seconds]

set -u

BUN_TESTS=/Users/eevv/focus/bun/test
BUN_RS=/Users/eevv/focus/bun-rs/target/release/bun-rs
TIMEOUT_SEC="${2:-15}"
OUT_CSV=/Users/eevv/focus/bun-rs/target/bun-suite-results.csv
OUT_SUM=/Users/eevv/focus/bun-rs/target/bun-suite-summary.txt
SUBSET="${1:?usage: run-bun-suite.sh <subset> [timeout-sec]}"

mkdir -p /Users/eevv/focus/bun-rs/target
: > "$OUT_CSV"
echo "path,status,passed,failed,note" >> "$OUT_CSV"

LIST=$(mktemp /tmp/bun-suite.XXXXXX)
trap "rm -f $LIST" EXIT
if [ -d "$BUN_TESTS/$SUBSET" ]; then
  find "$BUN_TESTS/$SUBSET" -name '*.test.*' -type f | sort > "$LIST"
else
  find $BUN_TESTS/$SUBSET -maxdepth 0 -type f 2>/dev/null | sort > "$LIST"
fi

TOTAL=$(wc -l < "$LIST" | tr -d ' ')
PASS_FILES=0
FAIL_FILES=0
PARSE_ERR=0
LOAD_ERR=0
TIMEOUTS=0
TESTS_PASSED=0
TESTS_FAILED=0

echo "[harness] running $TOTAL files with ${TIMEOUT_SEC}s timeout each…" >&2

i=0
while IFS= read -r f; do
  [ -z "$f" ] && continue
  i=$((i+1))
  if [ $((i % 20)) -eq 0 ]; then
    echo "[harness]   $i/$TOTAL …" >&2
  fi
  rel="${f#$BUN_TESTS/}"
  out=$(perl -e 'alarm shift; exec @ARGV' "$TIMEOUT_SEC" "$BUN_RS" test "$f" 2>&1)
  rc=$?

  status=""
  passed=0
  failed=0
  note=""

  if [ "$rc" -eq 142 ] || [ "$rc" -eq 124 ] || [ "$rc" -eq 14 ]; then
    status=timeout
    TIMEOUTS=$((TIMEOUTS+1))
  elif echo "$out" | grep -qE "parse errors:"; then
    status=parse_err
    PARSE_ERR=$((PARSE_ERR+1))
    note=$(echo "$out" | grep -E "parse errors:|error" | head -1 | head -c 160)
  else
    # Look for the runner summary line.
    summary=$(echo "$out" | grep -E "^tests:" | tail -1)
    if [ -n "$summary" ]; then
      passed=$(echo "$summary" | sed -nE 's/.*tests:[[:space:]]+([0-9]+) passed.*/\1/p')
      failed=$(echo "$summary" | sed -nE 's/.*passed,[[:space:]]+([0-9]+) failed.*/\1/p')
      passed=${passed:-0}
      failed=${failed:-0}
      if [ "$failed" = "0" ] && [ "$passed" != "0" ]; then
        status=pass
        PASS_FILES=$((PASS_FILES+1))
      elif [ "$failed" != "0" ]; then
        status=fail
        FAIL_FILES=$((FAIL_FILES+1))
        note=$(echo "$out" | grep -E "✗|Expected|toBe|toEqual|toHave" | head -1 | head -c 160)
      else
        # 0 passed 0 failed — no tests executed
        status=load_err
        LOAD_ERR=$((LOAD_ERR+1))
        note="no tests ran"
      fi
      TESTS_PASSED=$((TESTS_PASSED + passed))
      TESTS_FAILED=$((TESTS_FAILED + failed))
    else
      status=load_err
      LOAD_ERR=$((LOAD_ERR+1))
      note=$(echo "$out" | tail -2 | head -1 | head -c 160)
    fi
  fi

  # Strip commas/newlines from note for CSV safety.
  note=$(echo "$note" | tr ',\n' '  ')
  printf '"%s",%s,%s,%s,"%s"\n' "$rel" "$status" "$passed" "$failed" "$note" >> "$OUT_CSV"
done < "$LIST"

{
  echo "subset: $SUBSET"
  echo "files: total=$TOTAL  pass=$PASS_FILES  fail=$FAIL_FILES  parse_err=$PARSE_ERR  load_err=$LOAD_ERR  timeout=$TIMEOUTS"
  if [ "$TOTAL" -gt 0 ]; then
    pct=$(awk -v p="$PASS_FILES" -v t="$TOTAL" 'BEGIN { printf "%.1f", (p*100.0)/t }')
    echo "file_pass_rate: ${pct}%"
  fi
  echo "tests: passed=$TESTS_PASSED failed=$TESTS_FAILED"
} | tee "$OUT_SUM"
