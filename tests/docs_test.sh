#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)

fail() {
  printf 'FAIL: %s\n' "$1" >&2
  exit 1
}

section() {
  file=$1
  start=$2
  end=$3
  awk -v start="$start" -v end="$end" '
    $0 == start { printing = 1 }
    $0 == end { printing = 0 }
    printing { print }
  ' "$file"
}

assert_has() {
  text=$1
  needle=$2
  if ! printf '%s\n' "$text" | grep -F -q -- "$needle"; then
    fail "missing: $needle"
  fi
}

assert_lacks() {
  text=$1
  needle=$2
  if printf '%s\n' "$text" | grep -F -q -- "$needle"; then
    fail "should not contain: $needle"
  fi
}

en_install=$(section "$root/README.md" "## Install" "## Quick start")
zh_install=$(section "$root/README.zh-CN.md" "## 安装" "## 快速开始")
en_dev=$(section "$root/README.md" "## Development" "___END___")
zh_dev=$(section "$root/README.zh-CN.md" "## 开发" "___END___")

curl_line='curl --proto '"'"'=https'"'"' --tlsv1.2 -fsSL https://raw.githubusercontent.com/FFP-Tech-Lab/gv/main/install.sh | sh'
version_line='curl --proto '"'"'=https'"'"' --tlsv1.2 -fsSL https://raw.githubusercontent.com/FFP-Tech-Lab/gv/main/install.sh | GV_VERSION=0.1.0 sh'

for text in "$en_install" "$zh_install"; do
  assert_has "$text" "$curl_line"
  assert_has "$text" "$version_line"
  assert_has "$text" "~/.gv/bin/gv"
  assert_has "$text" "GV_VERSION"
  assert_lacks "$text" "cargo install --path ."
  assert_lacks "$text" "cargo build --release"
done

assert_has "$en_dev" "cargo test"
assert_has "$en_dev" "cargo build --release"
assert_has "$zh_dev" "cargo test"
assert_has "$zh_dev" "cargo build --release"

printf '%s\n' ok
