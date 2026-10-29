#!/usr/bin/env bash
# Runs the comparisons with the official server (the tests ignored as needing it, whose
# names say `official_server`), one test target at a time, each under a limit of its
# own, and says which passed, which failed and which ran out of time.
#
# Usage: tools/oracles.sh [filter ...]
#
# With filters, only the targets whose name contains one of them run
# (`projectiles_oracle raids`). CLUSTINE_ORACLE_LIMIT is the limit of one target in
# seconds, by default 1800; the comparisons of a whole game day have an hour. The
# caller agrees to the Minecraft EULA by setting CLUSTINE_ACCEPT_MINECRAFT_EULA=true, as
# for the tests themselves.
#
# A whole run once waited 22 hours on one scene, and another nearly six, with nothing
# saying so: the limit ends a target that hangs, and before it does the official
# server's threads are written into the target's log (`jstack`), beside the steps the
# scene printed (`--nocapture`), so that where it hung is known from the first time.
# Each target's line is printed as soon as it ends, for whoever watches the output.
set -u
cd "$(dirname "$0")/.."

filters=("$@")
# Not under TMPDIR, which the tests' worlds are kept in and which is cleared between
# runs: the logs of a run are what is left to look into afterwards.
mkdir -p target/oracle-runs
logs="$(mktemp -d target/oracle-runs/run.XXXXXX)"
failed=0

# Every test binary of the workspace, built once, as `manifest<TAB>kind<TAB>name<TAB>
# executable`; then those with a comparison, which the binary itself lists.
cargo test --workspace --locked --no-run --message-format=json >"${logs}/build.json" \
  2>"${logs}/build.log"
code=$?
if [ "$code" -ne 0 ]; then
  printf 'FAILED  build (exit %s, %s/build.log)\n' "$code" "$logs"
  exit 1
fi
python3 -I - "${logs}/build.json" >"${logs}/targets.tsv" <<'EOF'
import json, sys
for line in open(sys.argv[1]):
    try:
        message = json.loads(line)
    except ValueError:
        continue
    if message.get("reason") != "compiler-artifact" or not message.get("executable"):
        continue
    if not message["profile"]["test"]:
        continue
    target = message["target"]
    print("\t".join([message["manifest_path"], target["kind"][0], target["name"],
                     message["executable"]]))
EOF

# The processes started by `pid`, and theirs, deepest last.
descendants() {
  local children
  children="$(ps -o pid= --ppid "$1" 2>/dev/null)"
  for child in $children; do
    printf '%s\n' "$child"
    descendants "$child"
  done
}

while IFS=$'\t' read -r manifest kind name executable; do
  if [ "${#filters[@]}" -gt 0 ]; then
    wanted=0
    for filter in "${filters[@]}"; do
      case "$name" in *"$filter"*) wanted=1 ;; esac
    done
    [ "$wanted" -eq 1 ] || continue
  fi
  case "$name" in
    spawning_oracle | growth_oracle) limit="${CLUSTINE_ORACLE_LIMIT:-3600}" ;;
    *) limit="${CLUSTINE_ORACLE_LIMIT:-1800}" ;;
  esac
  if ! "$executable" --list --ignored official_server 2>/dev/null | grep -q ': test$'; then
    continue
  fi
  case "$kind" in
    lib) selector=(--lib) ;;
    bin) selector=(--bin "$name") ;;
    *) selector=(--test "$name") ;;
  esac
  log="${logs}/${name}.log"
  cargo test --manifest-path "$manifest" --locked "${selector[@]}" -- --ignored \
    official_server --test-threads 1 --nocapture >"$log" 2>&1 &
  pid=$!
  waited=0
  while kill -0 "$pid" 2>/dev/null && [ "$waited" -lt "$limit" ]; do
    sleep 5
    waited=$((waited + 5))
  done
  if kill -0 "$pid" 2>/dev/null; then
    started="$(descendants "$pid")"
    for process in $started; do
      if [ "$(ps -o comm= -p "$process" 2>/dev/null)" = java ]; then
        printf '\n==== the official server (%s) when the limit came\n' "$process" >>"$log"
        jstack "$process" >>"$log" 2>&1 || printf 'no jstack on the path\n' >>"$log"
        # At once and for good: a server stopped politely saves its world while it
        # goes, into the directory the next target's server has been given by then.
        kill -9 "$process" 2>/dev/null
      fi
    done
    kill $started "$pid" 2>/dev/null
    sleep 10
    kill -9 $started "$pid" 2>/dev/null
    wait "$pid" 2>/dev/null
    printf 'TIMEOUT %s (after %ss, %s)\n' "$name" "$limit" "$log"
    failed=1
  else
    wait "$pid"
    code=$?
    if [ "$code" -eq 0 ]; then
      printf 'ok      %s\n' "$name"
    else
      printf 'FAILED  %s (exit %s, %s)\n' "$name" "$code" "$log"
      grep -a -E '^test .* FAILED$' "$log" | sort -u | sed 's/^/          /'
      failed=1
    fi
  fi
done <"${logs}/targets.tsv"

printf 'logs: %s\n' "$logs"
exit "$failed"
