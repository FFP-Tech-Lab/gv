#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
ci=$root/.github/workflows/ci.yml
release=$root/.github/workflows/release.yml

fail() {
  printf 'FAIL: %s\n' "$1" >&2
  exit 1
}

if [ ! -f "$ci" ]; then
  fail "missing $ci"
fi
if [ ! -f "$release" ]; then
  fail "missing $release"
fi

require_in() {
  file=$1
  text=$2
  if ! grep -F -q -- "$text" "$file"; then
    fail "$(basename "$file") missing: $text"
  fi
}

require_in "$ci" "name: CI"
require_in "$ci" "branches: [main]"
require_in "$ci" "pull_request:"
require_in "$ci" "contents: read"
require_in "$ci" "cancel-in-progress: true"
require_in "$ci" "ubuntu-latest"
require_in "$ci" "macos-latest"
require_in "$ci" "actions/checkout@v5"
require_in "$ci" "rust-toolchain.toml"
require_in "$ci" "rustup toolchain install"
require_in "$ci" '"$channel" --profile minimal --no-self-update'
require_in "$ci" "cargo test --locked"
require_in "$ci" "sh tests/install_test.sh"
require_in "$ci" "sh tests/docs_test.sh"
require_in "$ci" "sh tests/workflows_test.sh"
require_in "$ci" "sh -n install.sh"
require_in "$ci" "runner.os == 'Linux'"

require_in "$release" "tags:"
require_in "$release" '- "v*"'
require_in "$release" "contents: write"
require_in "$release" "actions/checkout@v5"
require_in "$release" "rust-toolchain.toml"
require_in "$release" "rustup toolchain install"
require_in "$release" '"$channel" --profile minimal --no-self-update'
require_in "$release" "cargo metadata --no-deps --format-version 1"
require_in "$release" 'select(.name == "gv")'
require_in "$release" 'tag ${GITHUB_REF_NAME} does not match Cargo.toml version ${version}'
require_in "$release" "needs: version"
require_in "$release" "x86_64-unknown-linux-musl"
require_in "$release" "aarch64-unknown-linux-musl"
require_in "$release" "x86_64-apple-darwin"
require_in "$release" "aarch64-apple-darwin"
require_in "$release" "ubuntu-latest"
require_in "$release" "ubuntu-24.04-arm"
require_in "$release" "macos-latest"
require_in "$release" "gv-linux-amd64.tar.gz"
require_in "$release" "gv-linux-arm64.tar.gz"
require_in "$release" "gv-darwin-amd64.tar.gz"
require_in "$release" "gv-darwin-arm64.tar.gz"
require_in "$release" "musl-tools"
require_in "$release" "cargo build --release --locked --target"
require_in "$release" "COPYFILE_DISABLE=1"
require_in "$release" "actions/upload-artifact@v6"
require_in "$release" "actions/download-artifact@v6"
require_in "$release" "merge-multiple: true"
require_in "$release" "sha256sum"
require_in "$release" "SHA256SUMS"
require_in "$release" "gh release create"
require_in "$release" "--generate-notes"
require_in "$release" "--title"
require_in "$release" "GH_TOKEN: \${{ github.token }}"
require_in "$release" "needs: build"

if grep -F -q -- "cancel-in-progress" "$release"; then
  fail "release workflow must not cancel in progress"
fi
if grep -F -q -- "--clobber" "$release"; then
  fail "release workflow must not replace an existing release"
fi

printf '%s\n' ok
