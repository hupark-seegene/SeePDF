#!/usr/bin/env bash
# Runs a CI command and, when it fails, republishes the interesting part of its output as
# GitHub annotations (`::error::`).
#
# Why: job logs of a public repository still need a signed-in user to download, but check-run
# annotations are readable anonymously (GET /repos/{o}/{r}/check-runs/{id}/annotations). With
# this wrapper a failing compile, test or bundle step can be diagnosed from the API alone.
#
# Usage: bash scripts/ci-run.sh <title> <command> [args...]
set -uo pipefail

title="$1"
shift
log="${RUNNER_TEMP:-/tmp}/ci-run-$$.log"

"$@" 2>&1 | tee "$log"
status=${PIPESTATUS[0]}
if [ "$status" -eq 0 ]; then
  exit 0
fi

# Annotation text: % and newlines must be escaped (and `:` / `,` in the title property);
# GitHub keeps at most 10 error annotations per step. Colour codes are stripped first
# (CARGO_TERM_COLOR=always).
plain="${log}.plain"
sed -e 's/\x1b\[[0-9;]*[A-Za-z]//g' -e 's/\r//g' "$log" > "$plain"
escape() { sed -e 's/%/%25/g' | awk 'BEGIN{ORS="%0A"} {print}'; }
emit() {
  local body
  body="$(escape)"
  [ -n "$body" ] || return 0
  local t="${title} - $1"
  t="${t//%/%25}"
  t="${t//:/%3A}"
  t="${t//,/%2C}"
  printf '::error title=%s::%s\n' "$t" "$body"
}

# 1. rustc / cargo / npm diagnostics and test panics, one line each
grep -nE '^(error(\[E[0-9]+\])?:|warning: unused)|panicked at|^---- .* stdout ----|^failures:|^test .* FAILED|FAILED|Error:|error:|ERROR|failed to|Failed to' "$plain" \
  | head -n 40 | cut -c1-400 | emit "error lines"
# 2. the first rustc error with its span and notes
awk '/^error(\[E[0-9]+\])?:/{p=1} p{print; n++} n>=40{exit}' "$plain" | cut -c1-300 | emit "first error"
# 3. the panics of failed tests
awk '/^---- .* stdout ----/{p=1} p{print; n++} n>=80{exit}' "$plain" | cut -c1-300 | emit "failed tests"
# 4. the tail of the output
tail -n 60 "$plain" | cut -c1-300 | emit "last 60 lines"
exit "$status"
