#!/usr/bin/env bash
# Runs the four checks that every commit has to pass (see CLAUDE.md) and says which of
# them failed, by exit code and with the tests that failed.
#
# Usage: tools/check.sh
#
# The two runs of the tests go at the same time: the second one, on a world pinned
# into three regions, skips the tests that take minutes with clusters of processes, so
# it does not take much from the first. Run one after the other they take much longer
# for the same result. The logs are kept in a directory that is named at the end.
set -u
cd "$(dirname "$0")/.."

logs="$(mktemp -d "${TMPDIR:-/tmp}/clustine-check.XXXXXX")"
failed=0

report() {
  local name="$1" code="$2"
  if [ "$code" -eq 0 ]; then
    printf 'ok      %s\n' "$name"
  else
    printf 'FAILED  %s (exit %s, %s/%s.log)\n' "$name" "$code" "$logs" "$name"
    grep -a -E '^test .* FAILED$' "${logs}/${name}.log" | sort -u | sed 's/^/          /'
    failed=1
  fi
}

cargo fmt --all --check >"${logs}/fmt.log" 2>&1
report fmt $?
cargo clippy --workspace --all-targets --locked -- -D warnings >"${logs}/clippy.log" 2>&1
report clippy $?

# Built once, before the two runs, so that neither waits for the other's build.
cargo test --workspace --locked --no-run >"${logs}/build.log" 2>&1
report build $?

cargo test --workspace --locked >"${logs}/tests.log" 2>&1 &
tests=$!
CLUSTINE_TEST_PINS=0,4 cargo test -p clustine --locked >"${logs}/pinned.log" 2>&1 &
pinned=$!
wait "$tests"
report tests $?
wait "$pinned"
report pinned $?

printf 'logs: %s\n' "$logs"
exit "$failed"
