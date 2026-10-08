#!/usr/bin/env bash
# Runs the five checks that every commit has to pass (see CLAUDE.md) and says which of
# them failed, by exit code and with the tests that failed.
#
# Usage: tools/check.sh
#
# The three runs of the tests go at the same time: the second and the third, on a world
# divided into three regions, skip the tests that take minutes with clusters of
# processes, so they do not take much from the first. Run one after the other they
# take much longer for the same result. In the third the regions ask the world store
# which chunks they hold instead of taking their stripes as given
# (docs/adr/0012-the-tick-on-chunks.md, section 8). The logs are kept in a directory
# that is named at the end.
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

# Built once, before the three runs, so that none waits for another's build.
cargo test --workspace --locked --no-run >"${logs}/build.log" 2>&1
report build $?

cargo test --workspace --locked >"${logs}/tests.log" 2>&1 &
tests=$!
CLUSTINE_TEST_BOUNDARIES=0,4 cargo test -p clustine --locked >"${logs}/boundaries.log" 2>&1 &
boundaries=$!
CLUSTINE_TEST_BOUNDARIES=0,4 CLUSTINE_TEST_ASK_THE_STORE=1 \
  cargo test -p clustine --locked >"${logs}/asking.log" 2>&1 &
asking=$!
wait "$tests"
report tests $?
wait "$boundaries"
report boundaries $?
wait "$asking"
report asking $?

printf 'logs: %s\n' "$logs"
exit "$failed"
