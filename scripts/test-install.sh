#!/bin/sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"

fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }
pass() { printf 'ok - %s\n' "$*"; }

LLMSH_INSTALL_TESTING=1 . "$ROOT/install.sh"

TMP_DIRS=""

new_tmpdir() {
  tmp="$(mktemp -d)"
  TMP_DIRS="${TMP_DIRS}${TMP_DIRS:+
}$tmp"
  printf '%s\n' "$tmp"
}

cleanup() {
  old_ifs="${IFS}"
  IFS='
'
  for dir in $TMP_DIRS; do
    [ -n "$dir" ] || continue
    rm -rf "$dir"
  done
  IFS="$old_ifs"
}

trap cleanup EXIT INT TERM HUP

make_fake_uname_dir() {
  dir="$1"
  sys="$2"
  arch="$3"
  mkdir -p "$dir"
  cat > "$dir/uname" <<EOF
#!/bin/sh
case "\$1" in
  -s) printf '%s\n' "$sys" ;;
  -m) printf '%s\n' "$arch" ;;
  *) exit 1 ;;
esac
EOF
  chmod +x "$dir/uname"
}

make_fake_darwin_tools() {
  dir="$1"
  log="$2"
  mkdir -p "$dir"
  make_fake_uname_dir "$dir" "Darwin" "arm64"
  cat > "$dir/xattr" <<EOF
#!/bin/sh
printf 'xattr %s\n' "\$*" >> "$log"
EOF
  cat > "$dir/codesign" <<EOF
#!/bin/sh
printf 'codesign %s\n' "\$*" >> "$log"
EOF
  chmod +x "$dir/xattr" "$dir/codesign"
}

make_fake_curl() {
  dir="$1"
  effective_url="$2"
  mkdir -p "$dir"
  cat > "$dir/curl" <<EOF
#!/bin/sh
printf '%s' '$effective_url'
EOF
  chmod +x "$dir/curl"
}

make_fake_checksum_tool() {
  dir="$1"
  tool="$2"
  log="$3"
  exit_code="$4"
  mkdir -p "$dir"
  cat > "$dir/$tool" <<EOF
#!/bin/sh
printf '%s %s\n' "$tool" "\$*" >> "$log"
exit "$exit_code"
EOF
  chmod +x "$dir/$tool"
}

make_release_archive() {
  base_dir="$1"
  version="$2"
  target="$3"
  log="$4"
  archive="$5"

  release_dir="$base_dir/llmsh-$version-$target"
  mkdir -p "$release_dir"
  cat > "$release_dir/llmsh" <<EOF
#!/bin/sh
printf '%s\n' "\$*" >> "$log"
exit 0
EOF
  chmod +x "$release_dir/llmsh"
  tar -C "$base_dir" -czf "$archive" "llmsh-$version-$target"
}

test_artifact_name() {
  got="$(artifact_name "v0.3.2" "x86_64-apple-darwin")"
  [ "$got" = "llmsh-v0.3.2-x86_64-apple-darwin.tar.gz" ] || fail "artifact name: $got"
  pass "artifact_name"
}

test_detect_target_linux() {
  tmp="$(new_tmpdir)"
  make_fake_uname_dir "$tmp/bin" "Linux" "x86_64"
  got="$(PATH="$tmp/bin:$PATH" detect_target)"
  [ "$got" = "x86_64-unknown-linux-gnu" ] || fail "detect linux target: $got"
  pass "detect_target linux"
}

test_detect_target_darwin() {
  tmp="$(new_tmpdir)"
  make_fake_uname_dir "$tmp/bin" "Darwin" "arm64"
  got="$(PATH="$tmp/bin:$PATH" detect_target)"
  [ "$got" = "aarch64-apple-darwin" ] || fail "detect darwin target: $got"
  pass "detect_target darwin"
}

test_latest_version_parses_release_tag_redirect() {
  tmp="$(new_tmpdir)"
  make_fake_curl "$tmp/bin" "https://github.com/example/custom/releases/tag/v1.2.3"
  got="$(PATH="$tmp/bin:$PATH" latest_version "example/custom")"
  [ "$got" = "v1.2.3" ] || fail "latest_version should parse tag redirect: $got"
  pass "latest_version parses tag redirect"
}

test_latest_version_rejects_non_tag_redirect() {
  tmp="$(new_tmpdir)"
  make_fake_curl "$tmp/bin" "https://github.com/example/custom/releases"
  if (
    PATH="$tmp/bin:$PATH"
    latest_version "example/custom"
  ) >/dev/null 2>&1; then
    fail "latest_version should reject non-tag redirect"
  fi
  pass "latest_version rejects non-tag redirect"
}

test_checksum_failure() {
  tmp="$(new_tmpdir)"
  printf 'bad  file.tar.gz\n' > "$tmp/SHA256SUMS"
  printf 'content\n' > "$tmp/file.tar.gz"
  if checksum_verify "$tmp/SHA256SUMS" "$tmp/file.tar.gz" >/dev/null 2>&1; then
    fail "checksum failure should fail"
  fi
  pass "checksum failure"
}

test_checksum_success_host() {
  tmp="$(new_tmpdir)"
  archive="$tmp/file.tar.gz"
  printf 'content\n' > "$archive"
  checksum_tool="$(checksum_tool)"
  case "$checksum_tool" in
    sha256sum)
      checksum="$(sha256sum "$archive" | awk '{print $1}')"
      ;;
    shasum)
      checksum="$(shasum -a 256 "$archive" | awk '{print $1}')"
      ;;
    *)
      fail "unsupported checksum tool: $checksum_tool"
      ;;
  esac
  printf '%s  %s\n' "$checksum" "file.tar.gz" > "$tmp/SHA256SUMS"
  checksum_verify "$tmp/SHA256SUMS" "$archive" || fail "checksum success should pass on host"
  pass "checksum success host"
}

test_checksum_success_sha256sum_override() {
  tmp="$(new_tmpdir)"
  archive="$tmp/file.tar.gz"
  log="$tmp/checksum.log"
  printf 'content\n' > "$archive"
  printf 'deadbeef  file.tar.gz\n' > "$tmp/SHA256SUMS"
  make_fake_checksum_tool "$tmp/bin" "sha256sum" "$log" 0
  LLMSH_CHECKSUM_TOOL=sha256sum PATH="$tmp/bin:$PATH" checksum_verify "$tmp/SHA256SUMS" "$archive" || fail "sha256sum override should pass"
  grep -q '^sha256sum -c ' "$log" || fail "sha256sum override should invoke sha256sum -c"
  pass "checksum success sha256sum override"
}

test_checksum_success_shasum_override() {
  tmp="$(new_tmpdir)"
  archive="$tmp/file.tar.gz"
  log="$tmp/checksum.log"
  printf 'content\n' > "$archive"
  printf 'deadbeef  file.tar.gz\n' > "$tmp/SHA256SUMS"
  make_fake_checksum_tool "$tmp/bin" "shasum" "$log" 0
  LLMSH_CHECKSUM_TOOL=shasum PATH="$tmp/bin:$PATH" checksum_verify "$tmp/SHA256SUMS" "$archive" || fail "shasum override should pass"
  grep -q '^shasum -a 256 -c ' "$log" || fail "shasum override should invoke shasum -a 256 -c"
  pass "checksum success shasum override"
}

test_install_binary_darwin_postcopy() {
  tmp="$(new_tmpdir)"
  log="$tmp/tools.log"
  src="$tmp/src-llmsh"
  dest="$tmp/bin/llmsh"
  printf '#!/bin/sh\nexit 0\n' > "$src"
  chmod +x "$src"
  make_fake_darwin_tools "$tmp/bin-tools" "$log"

  PATH="$tmp/bin-tools:$PATH" install_binary "$src" "$dest"

  [ -x "$dest" ] || fail "install_binary should create executable"
  grep -q "xattr -c $dest" "$log" || fail "install_binary should clear xattr on macOS"
  grep -q "codesign --force --sign - $dest" "$log" || fail "install_binary should codesign on macOS"
  pass "install_binary darwin postcopy"
}

test_main_installs_and_runs_setup() {
  tmp="$(new_tmpdir)"
  install_dir="$tmp/install"
  log="$tmp/run.log"
  archive_root="$tmp/archive-root"
  version="v0.9.1"
  target="x86_64-unknown-linux-gnu"
  make_release_archive "$archive_root" "$version" "$target" "$log" "$tmp/archive.tar.gz"

  latest_version() { printf '%s\n' "${version#v}"; }
  detect_target() { printf '%s\n' "$target"; }
  stdin_is_interactive() { return 0; }
  download() {
    case "$1" in
      */SHA256SUMS) printf 'unused  archive.tar.gz\n' > "$2" ;;
      *) cp "$tmp/archive.tar.gz" "$2" ;;
    esac
  }
  checksum_verify() { :; }

  (
    LLMSH_REPO='example/custom'
    LLMSH_INSTALL_DIR="$install_dir"
    PATH="$PATH"
    main
  )

  [ -x "$install_dir/llmsh" ] || fail "main should install llmsh"
  grep -qx -- '--version' "$log" || fail "main should run --version"
  grep -qx -- 'setup' "$log" || fail "main should run setup"
  pass "main installs and runs setup"
}

test_main_skips_setup_when_requested() {
  tmp="$(new_tmpdir)"
  install_dir="$tmp/install"
  log="$tmp/run.log"
  archive_root="$tmp/archive-root"
  version="v1.2.3"
  target="x86_64-unknown-linux-gnu"
  make_release_archive "$archive_root" "$version" "$target" "$log" "$tmp/archive.tar.gz"

  latest_version() { printf '%s\n' "$version"; }
  detect_target() { printf '%s\n' "$target"; }
  stdin_is_interactive() { return 0; }
  download() {
    case "$1" in
      */SHA256SUMS) printf 'unused  archive.tar.gz\n' > "$2" ;;
      *) cp "$tmp/archive.tar.gz" "$2" ;;
    esac
  }
  checksum_verify() { :; }

  (
    LLMSH_INSTALL_DIR="$install_dir"
    LLMSH_SKIP_SETUP=1
    PATH="$PATH"
    main
  )

  [ -x "$install_dir/llmsh" ] || fail "main should install llmsh when setup skipped"
  grep -qx -- '--version' "$log" || fail "main should still run --version"
  if grep -qx -- 'setup' "$log"; then
    fail "main should skip setup when LLMSH_SKIP_SETUP=1"
  fi
  pass "main skips setup"
}

test_main_fails_when_archive_layout_is_wrong() {
  tmp="$(new_tmpdir)"
  install_dir="$tmp/install"
  archive_root="$tmp/archive-root"
  log="$tmp/run.log"
  mkdir -p "$archive_root/pkg"
  printf '#!/bin/sh\nexit 0\n' > "$archive_root/pkg/llmsh"
  chmod +x "$archive_root/pkg/llmsh"
  tar -C "$archive_root" -czf "$tmp/archive.tar.gz" pkg

  latest_version() { printf 'v2.0.0\n'; }
  detect_target() { printf 'x86_64-unknown-linux-gnu\n'; }
  stdin_is_interactive() { return 1; }
  download() {
    case "$1" in
      */SHA256SUMS) printf 'unused  archive.tar.gz\n' > "$2" ;;
      *) cp "$tmp/archive.tar.gz" "$2" ;;
    esac
  }
  checksum_verify() { :; }

  if (
    LLMSH_INSTALL_DIR="$install_dir"
    PATH="$PATH"
    main
  ) >"$log" 2>&1; then
    fail "main should fail when archive layout is wrong"
  fi

  grep -q 'expected release layout' "$log" || fail "main should report expected release layout"
  pass "main rejects wrong archive layout"
}

test_artifact_name
test_detect_target_linux
test_detect_target_darwin
test_latest_version_parses_release_tag_redirect
test_latest_version_rejects_non_tag_redirect
test_checksum_failure
test_checksum_success_host
test_checksum_success_sha256sum_override
test_checksum_success_shasum_override
test_install_binary_darwin_postcopy
test_main_installs_and_runs_setup
test_main_skips_setup_when_requested
test_main_fails_when_archive_layout_is_wrong
