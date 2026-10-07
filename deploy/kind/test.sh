#!/usr/bin/env bash
#
# The cluster test. Builds the image, starts a Kubernetes cluster in Docker with kind,
# deploys Clustine on it with two workers and lets bots walk back and forth across the
# boundary between them. It passes if no bot was disconnected, a watching bot saw each
# walker as exactly one entity throughout, and both workers handed players over.
#
# Usage: deploy/kind/test.sh [--reuse]
#
#   --reuse   If the cluster of an earlier run is still there (see KEEP), use it instead
#             of replacing it, which saves the time it takes to start one. What was
#             deployed on it is removed first.
#
# Environment:
#   KEEP=1            Leave the cluster running at the end, for inspection. Otherwise it
#                     is deleted, also when the test fails.
#   SKIP_BUILD=1      Do not build the image if clustine:dev exists already.
#   KIND=/path/kind   The kind to use. Otherwise the one in PATH, otherwise
#                     target/tools/kind, which get-kind.sh downloads.
#   ROLLOUT_TIMEOUT   Seconds to wait for each service to become ready (default 180).
#   BOTS_TIMEOUT      Seconds to wait for the bots to finish (default 300).

set -euo pipefail
# With CDPATH set, `cd` may print where it went, which would end up in $root below.
unset CDPATH
# One stream for everything, so that what this script says and what the tools say stay
# in order where the two streams are collected separately, as in CI.
exec 2>&1

readonly cluster=clustine-test
readonly context="kind-${cluster}"
readonly namespace=clustine
readonly image=clustine:dev

root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
readonly root

# kind adds the cluster to the kubeconfig and makes it the current context. In a file of
# this test's own, that leaves alone whichever cluster `kubectl` was talking to before.
export KUBECONFIG="${root}/target/kind/kubeconfig"

step() {
  printf '\n==> %s\n' "$*"
}

die() {
  printf 'test.sh: %s\n' "$*" >&2
  exit 1
}

# kubectl for the test's cluster and namespace, whatever the current context is.
k() {
  kubectl --context "$context" --namespace "$namespace" "$@"
}

reuse=0
for argument in "$@"; do
  case "$argument" in
    --reuse) reuse=1 ;;
    -h | --help)
      # The comment at the top of this file, without the shebang line.
      sed -n '3,/^$/ s/^# \{0,1\}//p' "${BASH_SOURCE[0]}"
      exit 0
      ;;
    *) die "unknown argument '${argument}'; see --help" ;;
  esac
done

keep="${KEEP:-0}"
skip_build="${SKIP_BUILD:-0}"
rollout_timeout="${ROLLOUT_TIMEOUT:-180}"
bots_timeout="${BOTS_TIMEOUT:-300}"
for value in "$rollout_timeout" "$bots_timeout"; do
  case "$value" in
    '' | *[!0-9]*)
      die "ROLLOUT_TIMEOUT and BOTS_TIMEOUT have to be numbers of seconds, which '${value}' is not"
      ;;
  esac
done

if [ -n "${KIND:-}" ]; then
  kind="$KIND"
  if ! command -v "$kind" >/dev/null 2>&1; then
    die "KIND is '${kind}', which is not a program"
  fi
elif command -v kind >/dev/null 2>&1; then
  kind=kind
elif [ -x "${root}/target/tools/kind" ]; then
  kind="${root}/target/tools/kind"
else
  die "kind was not found. Run deploy/kind/get-kind.sh, which downloads it to target/tools/kind, or set KIND."
fi

for tool in docker kubectl; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    die "${tool} is needed and was not found"
  fi
done

# Set as the test gets on, so that the end knows what there is to show and to remove.
cluster_touched=0
cluster_up=0
scratch=""

# Shows what a failed run left behind: nobody can look at a cluster that is deleted a
# moment later, and in CI nobody can look at it at all.
diagnostics() {
  local pod previous
  while IFS= read -r pod; do
    step "Last 100 log lines of ${pod}"
    k logs "$pod" --all-containers --tail=100
    # A container that was restarted has a new log; why it ended is in the old one.
    if previous="$(k logs "$pod" --all-containers --previous --tail=100 2>/dev/null)" &&
      [ -n "$previous" ]; then
      step "Last 100 log lines of ${pod} before it was restarted"
      printf '%s\n' "$previous"
    fi
  done < <(k get pods --output name)

  # Why a pod never started is not in any log but here.
  step "Events"
  k get events --sort-by=.lastTimestamp | tail -n 50

  step "Pods"
  k get pods --output wide
}

finish() {
  local status=$?
  # From here on nothing may end the script early or change how it ends.
  set +e
  trap - EXIT

  if [ "$status" -ne 0 ] && [ "$cluster_up" = 1 ]; then
    diagnostics
  fi

  if [ "$cluster_touched" = 1 ]; then
    if [ "$keep" = 1 ]; then
      step "The cluster ${cluster} is left running (KEEP=1)"
      printf 'To look at it:\n'
      printf '    kubectl --kubeconfig %s --namespace %s get pods\n' "$KUBECONFIG" "$namespace"
      printf 'To delete it:\n'
      printf '    %s delete cluster --name %s\n' "$kind" "$cluster"
    else
      step "Deleting the cluster ${cluster}"
      "$kind" delete cluster --name "$cluster"
    fi
  fi

  if [ -n "$scratch" ]; then
    rm -rf -- "$scratch"
  fi

  if [ "$status" -eq 0 ]; then
    step "The cluster test passed"
  else
    step "The cluster test FAILED"
  fi
  exit "$status"
}
trap finish EXIT
# An interrupted run cleans up like a failed one.
trap 'exit 130' INT
trap 'exit 143' TERM

scratch="$(mktemp -d)"
mkdir -p "$(dirname -- "$KUBECONFIG")"
cd -- "$root"

if [ "$skip_build" = 1 ] && docker image inspect "$image" >/dev/null 2>&1; then
  step "Using the image ${image} that exists already (SKIP_BUILD=1)"
else
  step "Building the image ${image}"
  if docker buildx version >/dev/null 2>&1; then
    # --load puts the image into Docker's own store also when the builder in use is not
    # the default one. Without attestations the image is a single manifest, the plainest
    # thing to hand to `kind load`.
    BUILDX_NO_DEFAULT_ATTESTATIONS=1 docker buildx build --load --tag "$image" "$root"
  else
    # Where the buildx plugin is not installed, Docker's older builder does it, with a
    # warning that it is deprecated. The Dockerfile needs nothing it does not have.
    docker build --tag "$image" "$root"
  fi
fi

# Not `kind get clusters | grep -q`: grep leaves at the first match, and the pipeline
# then counts as failed if kind had more to say.
clusters="$("$kind" get clusters)"
if grep -Fqx -- "$cluster" <<<"$clusters"; then
  cluster_exists=1
else
  cluster_exists=0
fi

if [ "$cluster_exists" = 1 ] && [ "$reuse" = 0 ]; then
  step "Deleting the cluster ${cluster} of an earlier run (--reuse would keep it)"
  "$kind" delete cluster --name "$cluster"
  cluster_exists=0
fi

cluster_touched=1
if [ "$cluster_exists" = 0 ]; then
  step "Creating the cluster ${cluster}"
  "$kind" create cluster --name "$cluster" --config "${root}/deploy/kind/cluster.yaml" --wait 120s
  cluster_up=1
else
  step "Reusing the cluster ${cluster}"
  # The kubeconfig of this test may be gone, for instance with the target directory.
  "$kind" export kubeconfig --name "$cluster"
  cluster_up=1
  # Pods of an earlier run would go on running the image they were started with, and
  # the world store would still have that run's world.
  k delete namespace "$namespace" --ignore-not-found --wait --timeout=120s
fi

step "Loading the image into the cluster"
"$kind" load docker-image "$image" --name "$cluster"

step "Deploying"
k apply --kustomize "${root}/deploy/kubernetes"

step "Waiting for the services"
# In no particular order but for the edge, which is last: it becomes ready only once
# every region has a worker, so its being ready is the whole cluster being ready.
for workload in \
  deployment/clustine-coordinator \
  statefulset/clustine-worldstore \
  statefulset/clustine-worker \
  deployment/clustine-edge; do
  k rollout status "$workload" --timeout="${rollout_timeout}s"
done

step "Running the bots"
# Foreground, so that the pod of an earlier Job is gone too before the new one starts.
k delete job clustine-bots --ignore-not-found --cascade=foreground --wait --timeout=60s
k apply --filename "${root}/deploy/kubernetes/test/bots.yaml"

# `kubectl wait` waits for one condition, and waiting for "Complete" alone would sit out
# the whole timeout when the bots have failed in the first seconds. So ask for both.
result=timeout
deadline=$((SECONDS + bots_timeout))
while [ "$SECONDS" -lt "$deadline" ]; do
  # A request that fails once is asked again; nothing but the deadline ends the wait.
  conditions="$(k get job clustine-bots \
    --output 'jsonpath={range .status.conditions[*]}{.type}={.status}{"\n"}{end}' \
    2>/dev/null || true)"
  if grep -Fqx 'Complete=True' <<<"$conditions"; then
    result=complete
    break
  fi
  if grep -Fqx 'Failed=True' <<<"$conditions"; then
    result=failed
    break
  fi
  sleep 2
done

step "Log of the bots"
k logs job/clustine-bots || true

case "$result" in
  complete) ;;
  failed) die "the bots failed; their log is above" ;;
  *) die "the bots did not finish within ${bots_timeout} seconds" ;;
esac

step "Checking that both workers handed players over"
# The bots would be just as content with a world that one worker runs alone. That each
# worker saw players arrive and saw players leave is what shows that two took part.
silent=0
for pod in clustine-worker-0 clustine-worker-1; do
  k logs "$pod" >"${scratch}/${pod}.log"
  for line in 'player arrived from another region' 'player departed to another region'; do
    # grep prints the count and fails if it is 0.
    count="$(grep -c -F -- "$line" "${scratch}/${pod}.log" || true)"
    printf '%s logged "%s" %s times\n' "$pod" "$line" "$count"
    # Written so that anything but a count of at least one is a failure.
    if ! [ "$count" -ge 1 ]; then
      silent=1
    fi
  done
done
if [ "$silent" = 1 ]; then
  die "not every worker saw players arrive and leave, so the bots did not cross between two workers"
fi
