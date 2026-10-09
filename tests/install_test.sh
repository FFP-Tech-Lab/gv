#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
script=$root/install.sh
COPYFILE_DISABLE=1
export COPYFILE_DISABLE

if [ ! -f "$script" ]; then
  printf '%s\n' "missing $script" >&2
  exit 1
fi

fail() {
  printf 'FAIL: %s\n' "$1" >&2
  if [ -f "${stderr_file:-}" ]; then
    printf 'stderr: %s\n' "$(cat "$stderr_file")" >&2
  fi
  if [ -f "${stdout_file:-}" ]; then
    printf 'stdout: %s\n' "$(cat "$stdout_file")" >&2
  fi
  exit 1
}

hash_of() {
  if [ -x /usr/bin/sha256sum ]; then
    /usr/bin/sha256sum "$1" | awk '{print $1}'
  else
    /usr/bin/shasum -a 256 "$1" | awk '{print $1}'
  fi
}

file_mode() {
  if /usr/bin/stat -c '%a' "$1" >/dev/null 2>&1; then
    /usr/bin/stat -c '%a' "$1"
  else
    /usr/bin/stat -f '%OLp' "$1"
  fi
}

write_fakes() {
  cat > "$fake/uname" <<'EOF'
#!/bin/sh
if [ "$1" = "-s" ]; then
  printf '%s\n' "$FAKE_UNAME_S"
elif [ "$1" = "-m" ]; then
  printf '%s\n' "$FAKE_UNAME_M"
else
  printf '%s\n' "uname: unsupported $1" >&2
  exit 1
fi
EOF
  cat > "$fake/curl" <<'EOF'
#!/bin/sh
out=
url=
saw_proto=0
saw_tls=0
saw_fs=0
prev=
for arg in "$@"; do
  if [ "$prev" = "-o" ]; then
    out=$arg
  fi
  if [ "$prev" = "--proto" ] && [ "$arg" = "=https" ]; then
    saw_proto=1
  fi
  if [ "$arg" = "--tlsv1.2" ]; then
    saw_tls=1
  fi
  if [ "$arg" = "-fsSL" ]; then
    saw_fs=1
  fi
  case $arg in
    https://*) url=$arg ;;
  esac
  prev=$arg
done
if [ "$saw_proto" -ne 1 ] || [ "$saw_tls" -ne 1 ] || [ "$saw_fs" -ne 1 ]; then
  printf '%s\n' "curl: required flags missing" >&2
  exit 2
fi
if [ -z "$url" ] || [ -z "$out" ]; then
  printf '%s\n' "curl: missing url or output" >&2
  exit 2
fi
printf '%s\n' "$url" >> "$CURL_LOG"
name=${url##*/}
if [ ! -f "$FIXTURE_DIR/$name" ]; then
  exit 22
fi
cp "$FIXTURE_DIR/$name" "$out"
EOF
  cat > "$fake/sha256sum" <<'EOF'
#!/bin/sh
if [ -x /usr/bin/sha256sum ]; then
  exec /usr/bin/sha256sum "$1"
fi
exec /usr/bin/shasum -a 256 "$1"
EOF
  cat > "$fake/shasum" <<'EOF'
#!/bin/sh
file=$3
if [ -x /usr/bin/sha256sum ]; then
  exec /usr/bin/sha256sum "$file"
fi
exec /usr/bin/shasum -a 256 "$file"
EOF
  chmod 0755 "$fake/uname" "$fake/curl" "$fake/sha256sum" "$fake/shasum"
}

link_essentials() {
  for cmd in mktemp tar mkdir mv chmod rm tr awk cp cat sed; do
    ln -s "$(command -v "$cmd")" "$ess/$cmd"
  done
}

begin_case() {
  tmp=$(mktemp -d)
  fake=$tmp/fake
  ess=$tmp/bin
  fixture=$tmp/fixture
  curl_log=$tmp/curl.log
  stdout_file=$tmp/stdout
  stderr_file=$tmp/stderr
  mkdir -p "$fake" "$ess" "$fixture"
  : > "$curl_log"
  write_fakes
  link_essentials
  path="$fake:$ess"
  include_sha256sum=1
  include_shasum=0
  include_curl=1
  fake_s=Linux
  fake_m=x86_64
  pass_home=1
  home_value=$tmp/home
  mkdir -p "$home_value"
  pass_root=0
  root_value=
  pass_version=0
  version_value=
}

apply_tool_flags() {
  if [ "$include_curl" -eq 0 ]; then
    rm -f "$fake/curl"
  fi
  if [ "$include_sha256sum" -eq 0 ]; then
    rm -f "$fake/sha256sum"
  fi
  if [ "$include_shasum" -eq 0 ]; then
    rm -f "$fake/shasum"
  fi
}

invoke() {
  apply_tool_flags
  set -- env -i \
    "PATH=$path" \
    "TMPDIR=$tmp" \
    "FAKE_UNAME_S=$fake_s" \
    "FAKE_UNAME_M=$fake_m" \
    "CURL_LOG=$curl_log" \
    "FIXTURE_DIR=$fixture" \
    "COPYFILE_DISABLE=1"
  if [ "$pass_home" -eq 1 ]; then
    set -- "$@" "HOME=$home_value"
  fi
  if [ "$pass_root" -eq 1 ]; then
    set -- "$@" "GV_ROOT=$root_value"
  fi
  if [ "$pass_version" -eq 1 ]; then
    set -- "$@" "GV_VERSION=$version_value"
  fi
  set -- "$@" /bin/sh "$script"
  set +e
  "$@" >"$stdout_file" 2>"$stderr_file"
  status=$?
  set -e
}

assert_status() {
  if [ "$status" -ne "$1" ]; then
    fail "expected status $1, got $status"
  fi
}

assert_stderr() {
  got=$(cat "$stderr_file")
  if [ "$got" != "$1" ]; then
    fail "expected stderr [$1]"
  fi
}

assert_stdout_empty() {
  if [ -s "$stdout_file" ]; then
    fail "expected empty stdout"
  fi
}

write_archive() {
  asset=$1
  body=$2
  member=$3
  printf '%s\n' "$body" > "$fixture/$member"
  tar -C "$fixture" -czf "$fixture/$asset" "$member"
  if [ "$member" != "gv" ]; then
    rm -f "$fixture/$member"
  fi
}

write_sums() {
  asset=$1
  digest=$2
  sep=$3
  {
    for name in gv-linux-amd64.tar.gz gv-linux-arm64.tar.gz gv-darwin-amd64.tar.gz gv-darwin-arm64.tar.gz; do
      if [ "$name" = "$asset" ]; then
        printf '%s%s%s\n' "$digest" "$sep" "$name"
      else
        printf '%s%s%s\n' "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb" "$sep" "$name"
      fi
    done
  } > "$fixture/SHA256SUMS"
}

expect_success() {
  asset=$1
  base=$2
  if [ "$pass_root" -eq 1 ]; then
    dest=$root_value/bin/gv
  else
    dest=$home_value/.gv/bin/gv
  fi
  assert_status 0
  assert_stderr ""
  expected=$(printf 'Installed gv to %s\neval "$(%s init bash)"\neval "$(%s init zsh)"' "$dest" "$dest" "$dest")
  got=$(cat "$stdout_file")
  if [ "$got" != "$expected" ]; then
    fail "unexpected stdout"
  fi
  if [ ! -f "$dest" ] || [ -L "$dest" ]; then
    fail "destination is not a regular file"
  fi
  if [ "$(file_mode "$dest")" != "755" ]; then
    fail "destination mode is not 755"
  fi
  if ! cmp -s "$dest" "$fixture/gv"; then
    fail "destination bytes differ"
  fi
  lines=$(wc -l < "$curl_log" | tr -d ' ')
  if [ "$lines" -ne 2 ]; then
    fail "expected 2 downloads, got $lines"
  fi
  first=$(sed -n '1p' "$curl_log")
  second=$(sed -n '2p' "$curl_log")
  if [ "$first" != "$base/$asset" ]; then
    fail "archive url [$first]"
  fi
  if [ "$second" != "$base/SHA256SUMS" ]; then
    fail "checksum url [$second]"
  fi
}

success_case() {
  os=$1
  mach=$2
  asset=$3
  mode=$4
  begin_case
  fake_s=$os
  fake_m=$mach
  case $mode in
    latest) ;;
    empty)
      pass_version=1
      version_value=
      ;;
    v)
      pass_version=1
      version_value=v0.1.0
      ;;
    bare)
      pass_version=1
      version_value=0.1.0
      ;;
    *)
      fail "bad mode $mode"
      ;;
  esac
  write_archive "$asset" "binary-$asset" gv
  digest=$(hash_of "$fixture/$asset")
  write_sums "$asset" "$digest" "  "
  if [ "$mode" = latest ] || [ "$mode" = empty ]; then
    base="https://github.com/FFP-Tech-Lab/gv/releases/latest/download"
  else
    base="https://github.com/FFP-Tech-Lab/gv/releases/download/v0.1.0"
  fi
  invoke
  expect_success "$asset" "$base"
  rm -rf "$tmp"
}

if [ ! -x "$script" ]; then
  fail "install.sh is not executable"
fi
/bin/sh -n "$script"

success_case Linux x86_64 gv-linux-amd64.tar.gz latest
success_case Linux amd64 gv-linux-amd64.tar.gz latest
success_case Linux aarch64 gv-linux-arm64.tar.gz latest
success_case Linux arm64 gv-linux-arm64.tar.gz latest
success_case Darwin x86_64 gv-darwin-amd64.tar.gz latest
success_case Darwin amd64 gv-darwin-amd64.tar.gz latest
success_case Darwin aarch64 gv-darwin-arm64.tar.gz v
success_case Darwin arm64 gv-darwin-arm64.tar.gz bare
success_case Linux x86_64 gv-linux-amd64.tar.gz empty

begin_case
pass_home=0
pass_root=1
root_value=$tmp/custom-root
write_archive gv-linux-amd64.tar.gz "rooted" gv
write_sums gv-linux-amd64.tar.gz "$(hash_of "$fixture/gv-linux-amd64.tar.gz")" "  "
invoke
expect_success gv-linux-amd64.tar.gz "https://github.com/FFP-Tech-Lab/gv/releases/latest/download"
rm -rf "$tmp"

begin_case
write_archive gv-linux-amd64.tar.gz "upper" gv
digest=$(hash_of "$fixture/gv-linux-amd64.tar.gz" | tr '[:lower:]' '[:upper:]')
write_sums gv-linux-amd64.tar.gz "$digest" "  "
invoke
expect_success gv-linux-amd64.tar.gz "https://github.com/FFP-Tech-Lab/gv/releases/latest/download"
rm -rf "$tmp"

begin_case
write_archive gv-linux-amd64.tar.gz "onespace" gv
write_sums gv-linux-amd64.tar.gz "$(hash_of "$fixture/gv-linux-amd64.tar.gz")" " "
invoke
expect_success gv-linux-amd64.tar.gz "https://github.com/FFP-Tech-Lab/gv/releases/latest/download"
rm -rf "$tmp"

begin_case
include_sha256sum=0
include_shasum=1
write_archive gv-linux-amd64.tar.gz "via-shasum" gv
write_sums gv-linux-amd64.tar.gz "$(hash_of "$fixture/gv-linux-amd64.tar.gz")" "  "
invoke
expect_success gv-linux-amd64.tar.gz "https://github.com/FFP-Tech-Lab/gv/releases/latest/download"
rm -rf "$tmp"

begin_case
pass_root=1
root_value=$tmp/linked
mkdir -p "$root_value/bin"
printf '%s\n' "untouched" > "$root_value/old-target"
ln -s "$root_value/old-target" "$root_value/bin/gv"
write_archive gv-linux-amd64.tar.gz "replaced" gv
write_sums gv-linux-amd64.tar.gz "$(hash_of "$fixture/gv-linux-amd64.tar.gz")" "  "
invoke
expect_success gv-linux-amd64.tar.gz "https://github.com/FFP-Tech-Lab/gv/releases/latest/download"
if [ "$(cat "$root_value/old-target")" != "untouched" ]; then
  fail "symlink target was overwritten"
fi
rm -rf "$tmp"

begin_case
fake_s=FreeBSD
fake_m=amd64
invoke
assert_status 1
assert_stderr "Unsupported platform FreeBSD amd64"
assert_stdout_empty
rm -rf "$tmp"

begin_case
pass_root=1
root_value=
invoke
assert_status 1
assert_stderr "GV_ROOT is empty"
assert_stdout_empty
rm -rf "$tmp"

begin_case
pass_home=0
invoke
assert_status 1
assert_stderr "HOME is not set"
assert_stdout_empty
rm -rf "$tmp"

begin_case
include_curl=0
invoke
assert_status 1
assert_stderr "curl is required"
assert_stdout_empty
rm -rf "$tmp"

begin_case
include_sha256sum=0
include_shasum=0
invoke
assert_status 1
assert_stderr "sha256sum or shasum is required"
assert_stdout_empty
rm -rf "$tmp"

begin_case
invoke
assert_status 1
assert_stderr "Download failed for gv-linux-amd64.tar.gz"
assert_stdout_empty
if [ -e "$home_value/.gv/bin/gv" ]; then
  fail "installed a binary after a failed download"
fi
rm -rf "$tmp"

begin_case
write_archive gv-linux-amd64.tar.gz "no-sums" gv
invoke
assert_status 1
assert_stderr "Download failed for SHA256SUMS"
assert_stdout_empty
rm -rf "$tmp"

begin_case
write_archive gv-linux-amd64.tar.gz "bad-sum" gv
write_sums gv-linux-amd64.tar.gz "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" "  "
invoke
assert_status 1
assert_stderr "Checksum mismatch for gv-linux-amd64.tar.gz"
assert_stdout_empty
if [ -e "$home_value/.gv/bin/gv" ]; then
  fail "installed a binary after a checksum mismatch"
fi
rm -rf "$tmp"

begin_case
write_archive gv-linux-amd64.tar.gz "dup" gv
digest=$(hash_of "$fixture/gv-linux-amd64.tar.gz")
write_sums gv-linux-amd64.tar.gz "$digest" "  "
printf '%s  %s\n' "$digest" gv-linux-amd64.tar.gz >> "$fixture/SHA256SUMS"
invoke
assert_status 1
assert_stderr "SHA256SUMS does not contain exactly one entry for gv-linux-amd64.tar.gz"
assert_stdout_empty
rm -rf "$tmp"

begin_case
write_archive gv-linux-amd64.tar.gz "none" gv
digest=$(hash_of "$fixture/gv-linux-amd64.tar.gz")
printf '%s  %s\n' "$digest" gv-linux-arm64.tar.gz > "$fixture/SHA256SUMS"
invoke
assert_status 1
assert_stderr "SHA256SUMS does not contain exactly one entry for gv-linux-amd64.tar.gz"
assert_stdout_empty
rm -rf "$tmp"

begin_case
write_archive gv-linux-amd64.tar.gz "not-gv" other
digest=$(hash_of "$fixture/gv-linux-amd64.tar.gz")
write_sums gv-linux-amd64.tar.gz "$digest" "  "
invoke
assert_status 1
assert_stderr "Archive is missing gv"
assert_stdout_empty
rm -rf "$tmp"

printf '%s\n' ok
