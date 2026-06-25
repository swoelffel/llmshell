#!/bin/sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"

fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }
pass() { printf 'ok - %s\n' "$*"; }

LLMSH_INSTALL_TESTING=1 . "$ROOT/install.sh"

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

test_artifact_name() {
  got="$(artifact_name "v0.3.2" "x86_64-apple-darwin")"
  [ "$got" = "llmsh-v0.3.2-x86_64-apple-darwin.tar.gz" ] || fail "artifact name: $got"
  pass "artifact_name"
}

test_detect_target_linux() {
  tmp="$(mktemp -d)"
  make_fake_uname_dir "$tmp/bin" "Linux" "x86_64"
  got="$(PATH="$tmp/bin:$PATH" detect_target)"
  [ "$got" = "x86_64-unknown-linux-gnu" ] || fail "detect linux target: $got"
  pass "detect_target linux"
}

test_detect_target_darwin() {
  tmp="$(mktemp -d)"
  make_fake_uname_dir "$tmp/bin" "Darwin" "arm64"
  got="$(PATH="$tmp/bin:$PATH" detect_target)"
  [ "$got" = "aarch64-apple-darwin" ] || fail "detect darwin target: $got"
  pass "detect_target darwin"
}

test_checksum_failure() {
  tmp="$(mktemp -d)"
  printf 'bad  file.tar.gz\n' > "$tmp/SHA256SUMS"
  printf 'content\n' > "$tmp/file.tar.gz"
  if checksum_verify "$tmp/SHA256SUMS" "$tmp/file.tar.gz" >/dev/null 2>&1; then
    fail "checksum failure should fail"
  fi
  pass "checksum failure"
}

test_install_binary_darwin_postcopy() {
  tmp="$(mktemp -d)"
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
  tmp="$(mktemp -d)"
  install_dir="$tmp/install"
  log="$tmp/run.log"
  archive_root="$tmp/archive-root"
  mkdir -p "$archive_root/pkg"
  cat > "$archive_root/pkg/llmsh" <<EOF
#!/bin/sh
printf '%s\n' "\$*" >> "$log"
exit 0
EOF
  chmod +x "$archive_root/pkg/llmsh"
  tar -C "$archive_root" -czf "$tmp/archive.tar.gz" pkg

  latest_version() { printf '0.9.1\n'; }
  detect_target() { printf 'x86_64-unknown-linux-gnu\n'; }
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
  tmp="$(mktemp -d)"
  install_dir="$tmp/install"
  log="$tmp/run.log"
  archive_root="$tmp/archive-root"
  mkdir -p "$archive_root/pkg"
  cat > "$archive_root/pkg/llmsh" <<EOF
#!/bin/sh
printf '%s\n' "\$*" >> "$log"
exit 0
EOF
  chmod +x "$archive_root/pkg/llmsh"
  tar -C "$archive_root" -czf "$tmp/archive.tar.gz" pkg

  latest_version() { printf 'v1.2.3\n'; }
  detect_target() { printf 'x86_64-unknown-linux-gnu\n'; }
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

test_artifact_name
test_detect_target_linux
test_detect_target_darwin
test_checksum_failure
test_install_binary_darwin_postcopy
test_main_installs_and_runs_setup
test_main_skips_setup_when_requested
