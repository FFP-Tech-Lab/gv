#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
ci=$root/.github/workflows/ci.yml

fail() {
  printf 'FAIL: %s\n' "$1" >&2
  exit 1
}

if [ ! -f "$ci" ]; then
  fail "missing $ci"
fi

require() {
  if ! grep -F -q -- "$1" "$ci"; then
    fail "ci.yml missing: $1"
  fi
}

require "name: CI"
require "branches: [main]"
require "pull_request:"
require "contents: read"
require "cancel-in-progress: true"
require "ubuntu-latest"
require "macos-latest"
require "actions/checkout@v5"
require "rust-toolchain.toml"
require "rustup toolchain install"
require '"$channel" --profile minimal --no-self-update'
require "cargo test --locked"
require "sh tests/install_test.sh"
require "sh tests/workflows_test.sh"
require "sh -n install.sh"
require "runner.os == 'Linux'"

printf '%s\n' ok
