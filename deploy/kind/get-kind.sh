#!/usr/bin/env bash
#
# Downloads kind (https://kind.sigs.k8s.io), which runs a Kubernetes cluster in Docker
# containers, to target/tools/kind in the repository and prints that path. Does nothing
# but print the path if the wanted version is there already.
#
# Usage: deploy/kind/get-kind.sh
#
# Environment:
#   KIND_VERSION   the release to get, for instance v0.31.0 (the default)

set -euo pipefail
# With CDPATH set, `cd` may print where it went, which would end up in $root below.
unset CDPATH

# A fixed release, so that the cluster test changes when this line does and not when a
# new kind comes out. Each release also decides which Kubernetes version it starts.
version="${KIND_VERSION:-v0.31.0}"

root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
directory="${root}/target/tools"
destination="${directory}/kind"

# Only the path goes to standard output, so that `KIND="$(deploy/kind/get-kind.sh)"`
# works; everything else is said on standard error.
say() {
  echo "get-kind.sh: $*" >&2
}

die() {
  say "$*"
  exit 1
}

# The version becomes part of a URL, so it is not taken on trust.
if ! [[ "$version" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  die "KIND_VERSION has to look like v0.31.0, which '${version}' does not"
fi

if [ "$(uname -s)" != Linux ]; then
  die "this script only gets kind for Linux; for other systems see https://kind.sigs.k8s.io/docs/user/quick-start/#installation"
fi

machine="$(uname -m)"
case "$machine" in
  x86_64 | amd64) architecture=amd64 ;;
  aarch64 | arm64) architecture=arm64 ;;
  *) die "there is no kind download for the architecture '${machine}' here; only x86_64 and aarch64" ;;
esac

# `kind version` prints, for instance, "kind v0.31.0 go1.25.5 linux/amd64". A file that
# is not kind or is for another architecture fails to run or prints something else, and
# is replaced.
if [ -x "$destination" ]; then
  installed="$("$destination" version 2>/dev/null || true)"
  case " ${installed} " in
    *" ${version} "*)
      echo "$destination"
      exit 0
      ;;
  esac
fi

for tool in curl sha256sum; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    die "${tool} is needed and was not found"
  fi
done

mkdir -p "$directory"
# Downloaded next to where it will be, so that the finished file can be moved into
# place in one step and a download that was interrupted never looks like kind.
work="$(mktemp -d "${directory}/get-kind.XXXXXX")"
trap 'rm -rf -- "$work"' EXIT

file="kind-linux-${architecture}"
url="https://kind.sigs.k8s.io/dl/${version}/${file}"

# The address redirects to the release on GitHub. HTTPS only, also after the redirect.
fetch() {
  curl --fail --silent --show-error --location \
    --proto '=https' --proto-redir '=https' \
    --retry 3 --output "$2" "$1"
}

say "downloading ${url}"
fetch "$url" "${work}/${file}"
fetch "${url}.sha256sum" "${work}/${file}.sha256sum"

# The published file has the format of sha256sum: the digest, then the file's name.
# Only the digest is taken from it, so that the name it gives does not matter. Both
# files come from the same place, so this catches a damaged or cut-off download and not
# a release that was tampered with.
expected="$(awk 'NR == 1 { print $1 }' "${work}/${file}.sha256sum")"
if ! [[ "$expected" =~ ^[0-9a-f]{64}$ ]]; then
  die "${url}.sha256sum does not begin with a SHA-256 digest"
fi
actual="$(sha256sum "${work}/${file}" | awk '{ print $1 }')"
if [ "$actual" != "$expected" ]; then
  die "the download has the SHA-256 digest ${actual}, but ${expected} was published; nothing was installed"
fi

chmod +x "${work}/${file}"
mv -f -- "${work}/${file}" "$destination"

say "kind ${version} is now at ${destination}"
echo "$destination"
