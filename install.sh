#!/bin/sh
set -eu

repo="https://github.com/FFP-Tech-Lab/gv"

if [ "${GV_ROOT+set}" = set ]; then
  if [ -z "$GV_ROOT" ]; then
    printf '%s\n' "GV_ROOT is empty" >&2
    exit 1
  fi
  root=$GV_ROOT
else
  if [ "${HOME+set}" != set ] || [ -z "$HOME" ]; then
    printf '%s\n' "HOME is not set" >&2
    exit 1
  fi
  root=$HOME/.gv
fi

os=$(uname -s)
mach=$(uname -m)
case "$os:$mach" in
  Linux:x86_64|Linux:amd64) asset=gv-linux-amd64.tar.gz ;;
  Linux:aarch64|Linux:arm64) asset=gv-linux-arm64.tar.gz ;;
  Darwin:x86_64|Darwin:amd64) asset=gv-darwin-amd64.tar.gz ;;
  Darwin:aarch64|Darwin:arm64) asset=gv-darwin-arm64.tar.gz ;;
  *)
    printf '%s\n' "Unsupported platform ${os} ${mach}" >&2
    exit 1
    ;;
esac

version=${GV_VERSION-}
if [ -n "$version" ]; then
  version=${version#v}
  base="${repo}/releases/download/v${version}"
else
  base="${repo}/releases/latest/download"
fi

if ! command -v curl >/dev/null 2>&1; then
  printf '%s\n' "curl is required" >&2
  exit 1
fi

if command -v sha256sum >/dev/null 2>&1; then
  hasher=sha256sum
elif command -v shasum >/dev/null 2>&1; then
  hasher=shasum
else
  printf '%s\n' "sha256sum or shasum is required" >&2
  exit 1
fi

if ! tmp=$(mktemp -d); then
  printf '%s\n' "Could not create a temporary directory" >&2
  exit 1
fi
trap 'rm -rf "$tmp"' EXIT

if ! curl --proto '=https' --tlsv1.2 -fsSL "${base}/${asset}" -o "$tmp/$asset" 2>/dev/null; then
  printf '%s\n' "Download failed for ${asset}" >&2
  exit 1
fi

if ! curl --proto '=https' --tlsv1.2 -fsSL "${base}/SHA256SUMS" -o "$tmp/SHA256SUMS" 2>/dev/null; then
  printf '%s\n' "Download failed for SHA256SUMS" >&2
  exit 1
fi

count=0
expected=
while IFS= read -r line || [ -n "$line" ]; do
  [ -n "$line" ] || continue
  digest=$(printf '%s\n' "$line" | awk '{print $1}')
  name=$(printf '%s\n' "$line" | awk '{print $2}')
  if [ "$name" = "$asset" ]; then
    count=$((count + 1))
    expected=$digest
  fi
done < "$tmp/SHA256SUMS"

if [ "$count" -ne 1 ]; then
  printf '%s\n' "SHA256SUMS does not contain exactly one entry for ${asset}" >&2
  exit 1
fi

if [ "$hasher" = sha256sum ]; then
  actual=$(sha256sum "$tmp/$asset" | awk '{print $1}')
else
  actual=$(shasum -a 256 "$tmp/$asset" | awk '{print $1}')
fi
expected=$(printf '%s' "$expected" | tr '[:upper:]' '[:lower:]')
actual=$(printf '%s' "$actual" | tr '[:upper:]' '[:lower:]')
if [ "$expected" != "$actual" ]; then
  rm -f "$tmp/$asset"
  printf '%s\n' "Checksum mismatch for ${asset}" >&2
  exit 1
fi

if ! tar -xzf "$tmp/$asset" -C "$tmp" gv 2>/dev/null || [ ! -f "$tmp/gv" ]; then
  printf '%s\n' "Archive is missing gv" >&2
  exit 1
fi

if ! mkdir -p "$root/bin"; then
  printf '%s\n' "Could not create ${root}/bin" >&2
  exit 1
fi

if ! stage=$(mktemp "$root/bin/gv.XXXXXX"); then
  printf '%s\n' "Could not install gv to ${root}/bin/gv" >&2
  exit 1
fi
if ! cp "$tmp/gv" "$stage" || ! chmod 0755 "$stage" || ! mv -f "$stage" "$root/bin/gv"; then
  rm -f "$stage"
  printf '%s\n' "Could not install gv to ${root}/bin/gv" >&2
  exit 1
fi

printf 'Installed gv to %s\n' "$root/bin/gv"
printf 'eval "$(%s init bash)"\n' "$root/bin/gv"
printf 'eval "$(%s init zsh)"\n' "$root/bin/gv"
