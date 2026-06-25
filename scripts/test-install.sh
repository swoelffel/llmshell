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

restore_install_functions() {
  LLMSH_INSTALL_TESTING=1 . "$ROOT/install.sh"
}

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

make_fake_download_curl() {
  dir="$1"
  mkdir -p "$dir"
  cat > "$dir/curl" <<'EOF'
#!/bin/sh
out=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    -o)
      shift
      out="$1"
      ;;
  esac
  shift
done
[ -n "$out" ] || exit 2
printf 'downloaded\n' > "$out"
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

test_download_does_not_clobber_installer_dest() {
  tmp="$(new_tmpdir)"
  make_fake_download_curl "$tmp/bin"
  dest="$tmp/install/llmsh"
  archive="$tmp/archive.tar.gz"

  PATH="$tmp/bin:$PATH" download "https://example.invalid/archive.tar.gz" "$archive"

  [ "$dest" = "$tmp/install/llmsh" ] || fail "download should not clobber installer dest variable: $dest"
  grep -q '^downloaded$' "$archive" || fail "download should write requested archive path"
  pass "download does not clobber installer dest"
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

test_setup_mode_interactive_stdin() {
  stdin_is_interactive() { return 0; }
  tty_is_available() { return 1; }

  got="$(setup_mode)"
  [ "$got" = "stdin" ] || fail "setup_mode should prefer interactive stdin: $got"
  pass "setup_mode prefers interactive stdin"
}

test_setup_mode_tty_fallback() {
  stdin_is_interactive() { return 1; }
  tty_is_available() { return 0; }

  got="$(setup_mode)"
  [ "$got" = "tty" ] || fail "setup_mode should use tty fallback: $got"
  pass "setup_mode uses tty fallback"
}

test_setup_mode_skip_env() {
  stdin_is_interactive() { return 0; }
  tty_is_available() { return 0; }

  got="$(LLMSH_SKIP_SETUP=1 setup_mode)"
  [ "$got" = "skip" ] || fail "setup_mode should honor LLMSH_SKIP_SETUP=1: $got"
  pass "setup_mode honors skip env"
}

test_setup_mode_without_terminal() {
  stdin_is_interactive() { return 1; }
  tty_is_available() { return 1; }

  got="$(setup_mode)"
  [ "$got" = "none" ] || fail "setup_mode should skip without terminal: $got"
  pass "setup_mode skips without terminal"
}

test_run_setup_skips_when_tty_open_fails() {
  tmp="$(new_tmpdir)"
  dest="$tmp/llmsh"
  stdout_log="$tmp/stdout.log"
  stderr_log="$tmp/stderr.log"

  cat > "$dest" <<'EOF'
#!/bin/sh
exit 0
EOF
  chmod +x "$dest"

  setup_mode() { printf '%s\n' "tty"; }
  tty_device() { printf '%s\n' "$tmp/missing-tty"; }

  if ! run_setup "$dest" >"$stdout_log" 2>"$stderr_log"; then
    fail "run_setup should not fail when tty redirection cannot be opened"
  fi

  [ ! -s "$stdout_log" ] || fail "run_setup should stay silent on stdout when tty setup fails"
  grep -q 'unable to open' "$stderr_log" || fail "run_setup should explain tty open failure"
  grep -q "Run 'llmsh setup' or 'llmsh' later" "$stderr_log" || fail "run_setup should explain how to continue after tty failure"
  pass "run_setup skips when tty open fails"
}

test_run_setup_fails_when_tty_setup_command_fails() {
  tmp="$(new_tmpdir)"
  dest="$tmp/llmsh"
  tty_file="$tmp/fake-tty"
  stderr_log="$tmp/stderr.log"
  : > "$tty_file"

  cat > "$dest" <<'EOF'
#!/bin/sh
exit 23
EOF
  chmod +x "$dest"

  setup_mode() { printf '%s\n' "tty"; }
  tty_device() { printf '%s\n' "$tty_file"; }

  if run_setup "$dest" 2>"$stderr_log"; then
    fail "run_setup should fail when setup exits nonzero in tty mode"
  else
    status=$?
  fi

  [ "$status" -eq 23 ] || fail "run_setup should preserve tty setup exit status: $status"
  [ ! -s "$stderr_log" ] || fail "run_setup should not print skip guidance for tty setup command failure"
  pass "run_setup fails when tty setup command fails"
}

test_path_contains_dir_matches_exact_entry() {
  PATH="/usr/bin:/tmp/llmsh/bin:/bin"
  path_contains_dir "/tmp/llmsh/bin" || fail "path_contains_dir should match exact PATH entry"
  if path_contains_dir "/tmp/llmsh"; then
    fail "path_contains_dir should not match partial PATH entry"
  fi
  pass "path_contains_dir matches exact entry"
}

test_print_path_guidance_when_missing_after_setup() {
  tmp="$(new_tmpdir)"
  install_dir="$tmp/bin"
  dest="$install_dir/llmsh"
  stdout_log="$tmp/stdout.log"
  stderr_log="$tmp/stderr.log"

  PATH="/usr/bin:/bin" print_path_guidance "$install_dir" "$dest" "stdin" >"$stdout_log" 2>"$stderr_log"

  [ ! -s "$stdout_log" ] || fail "print_path_guidance should not write to stdout"
  grep -q 'not on your PATH' "$stderr_log" || fail "print_path_guidance should explain PATH gap"
  grep -q "export PATH=\"$install_dir:\$PATH\"" "$stderr_log" || fail "print_path_guidance should print exact export command"
  grep -q "Setup already ran via '$dest setup'" "$stderr_log" || fail "print_path_guidance should mention absolute-path setup"
  pass "print_path_guidance reports missing PATH after setup"
}

test_print_path_guidance_is_silent_when_present() {
  tmp="$(new_tmpdir)"
  install_dir="$tmp/bin"
  dest="$install_dir/llmsh"
  stdout_log="$tmp/stdout.log"
  stderr_log="$tmp/stderr.log"

  PATH="$install_dir:/usr/bin:/bin" print_path_guidance "$install_dir" "$dest" "stdin" >"$stdout_log" 2>"$stderr_log"

  [ ! -s "$stdout_log" ] || fail "print_path_guidance should stay silent on stdout when PATH already contains install dir"
  [ ! -s "$stderr_log" ] || fail "print_path_guidance should stay silent when PATH already contains install dir"
  pass "print_path_guidance is silent when PATH already contains install dir"
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

test_main_runs_setup_via_tty_fallback() {
  tmp="$(new_tmpdir)"
  install_dir="$tmp/install"
  log="$tmp/run.log"
  archive_root="$tmp/archive-root"
  version="v1.1.0"
  target="x86_64-unknown-linux-gnu"
  tty_file="$tmp/fake-tty"
  : > "$tty_file"
  make_release_archive "$archive_root" "$version" "$target" "$log" "$tmp/archive.tar.gz"

  latest_version() { printf '%s\n' "$version"; }
  detect_target() { printf '%s\n' "$target"; }
  stdin_is_interactive() { return 1; }
  tty_is_available() { return 0; }
  tty_device() { printf '%s\n' "$tty_file"; }
  download() {
    case "$1" in
      */SHA256SUMS) printf 'unused  archive.tar.gz\n' > "$2" ;;
      *) cp "$tmp/archive.tar.gz" "$2" ;;
    esac
  }
  checksum_verify() { :; }

  (
    LLMSH_INSTALL_DIR="$install_dir"
    PATH="$PATH"
    main
  )

  [ -x "$install_dir/llmsh" ] || fail "main should install llmsh with tty fallback"
  grep -qx -- '--version' "$log" || fail "main should run --version before tty setup"
  grep -qx -- 'setup' "$log" || fail "main should run setup via tty fallback"
  pass "main runs setup via tty fallback"
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

test_main_skips_setup_without_terminal() {
  tmp="$(new_tmpdir)"
  install_dir="$tmp/install"
  log="$tmp/run.log"
  archive_root="$tmp/archive-root"
  version="v2.1.0"
  target="x86_64-unknown-linux-gnu"
  make_release_archive "$archive_root" "$version" "$target" "$log" "$tmp/archive.tar.gz"

  latest_version() { printf '%s\n' "$version"; }
  detect_target() { printf '%s\n' "$target"; }
  stdin_is_interactive() { return 1; }
  tty_is_available() { return 1; }
  download() {
    case "$1" in
      */SHA256SUMS) printf 'unused  archive.tar.gz\n' > "$2" ;;
      *) cp "$tmp/archive.tar.gz" "$2" ;;
    esac
  }
  checksum_verify() { :; }

  if ! (
    LLMSH_INSTALL_DIR="$install_dir"
    PATH="$PATH"
    main
  ) >"$tmp/stdout.log" 2>"$tmp/stderr.log"; then
    fail "main should not fail when setup is skipped without terminal"
  fi

  [ -x "$install_dir/llmsh" ] || fail "main should install llmsh without terminal"
  if grep -qx -- 'setup' "$log"; then
    fail "main should not run setup without terminal"
  fi
  grep -q 'Run '\''llmsh setup'\'' or '\''llmsh'\'' later' "$tmp/stderr.log" || fail "main should explain how to continue without terminal"
  pass "main skips setup without terminal"
}

test_main_reports_path_guidance_after_successful_setup() {
  tmp="$(new_tmpdir)"
  install_dir="$tmp/install"
  log="$tmp/run.log"
  archive_root="$tmp/archive-root"
  version="v2.2.0"
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

  if ! (
    LLMSH_INSTALL_DIR="$install_dir"
    PATH="/usr/bin:/bin"
    main
  ) >"$tmp/stdout.log" 2>"$tmp/stderr.log"; then
    fail "main should succeed when install dir is not on PATH"
  fi

  grep -qx -- '--version' "$log" || fail "main should run --version before PATH guidance"
  grep -qx -- 'setup' "$log" || fail "main should run setup before PATH guidance"
  grep -q "export PATH=\"$install_dir:\$PATH\"" "$tmp/stderr.log" || fail "main should print PATH export guidance"
  grep -q "Setup already ran via '$install_dir/llmsh setup'" "$tmp/stderr.log" || fail "main should mention absolute-path setup after PATH guidance"
  pass "main reports PATH guidance after setup"
}

test_main_skips_setup_when_tty_open_fails() {
  tmp="$(new_tmpdir)"
  install_dir="$tmp/install"
  log="$tmp/run.log"
  archive_root="$tmp/archive-root"
  version="v2.3.0"
  target="x86_64-unknown-linux-gnu"
  make_release_archive "$archive_root" "$version" "$target" "$log" "$tmp/archive.tar.gz"

  latest_version() { printf '%s\n' "$version"; }
  detect_target() { printf '%s\n' "$target"; }
  stdin_is_interactive() { return 1; }
  tty_is_available() { return 0; }
  tty_device() { printf '%s\n' "$tmp/missing-tty"; }
  download() {
    case "$1" in
      */SHA256SUMS) printf 'unused  archive.tar.gz\n' > "$2" ;;
      *) cp "$tmp/archive.tar.gz" "$2" ;;
    esac
  }
  checksum_verify() { :; }

  if ! (
    LLMSH_INSTALL_DIR="$install_dir"
    PATH="$PATH"
    main
  ) >"$tmp/stdout.log" 2>"$tmp/stderr.log"; then
    fail "main should not fail when tty setup redirection cannot be opened"
  fi

  [ -x "$install_dir/llmsh" ] || fail "main should still install llmsh when tty setup fails"
  grep -qx -- '--version' "$log" || fail "main should still run --version before tty setup fallback"
  if grep -qx -- 'setup' "$log"; then
    fail "main should not run setup when tty redirection cannot be opened"
  fi
  grep -q 'unable to open' "$tmp/stderr.log" || fail "main should explain tty open failure"
  pass "main skips setup when tty open fails"
}

test_main_fails_when_tty_setup_command_fails() {
  tmp="$(new_tmpdir)"
  install_dir="$tmp/install"
  archive_root="$tmp/archive-root"
  log="$tmp/run.log"
  version="v2.4.0"
  target="x86_64-unknown-linux-gnu"
  tty_file="$tmp/fake-tty"
  : > "$tty_file"

  release_dir="$archive_root/llmsh-$version-$target"
  mkdir -p "$release_dir"
  cat > "$release_dir/llmsh" <<EOF
#!/bin/sh
printf '%s\n' "\$*" >> "$log"
case "\$1" in
  --version) exit 0 ;;
  setup) exit 29 ;;
  *) exit 0 ;;
esac
EOF
  chmod +x "$release_dir/llmsh"
  tar -C "$archive_root" -czf "$tmp/archive.tar.gz" "llmsh-$version-$target"

  latest_version() { printf '%s\n' "$version"; }
  detect_target() { printf '%s\n' "$target"; }
  stdin_is_interactive() { return 1; }
  tty_is_available() { return 0; }
  tty_device() { printf '%s\n' "$tty_file"; }
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
  ) >"$tmp/stdout.log" 2>"$tmp/stderr.log"; then
    fail "main should fail when tty-mode setup exits nonzero"
  else
    status=$?
  fi

  [ "$status" -eq 29 ] || fail "main should preserve tty-mode setup exit status: $status"
  [ -x "$install_dir/llmsh" ] || fail "main should still install llmsh before tty setup fails"
  grep -qx -- '--version' "$log" || fail "main should run --version before tty-mode setup failure"
  grep -qx -- 'setup' "$log" || fail "main should attempt setup in tty mode"
  [ ! -s "$tmp/stderr.log" ] || fail "main should not print tty-open skip guidance when setup itself fails"
  pass "main fails when tty setup command fails"
}

run_test() {
  restore_install_functions
  "$1"
}

run_test test_artifact_name
run_test test_detect_target_linux
run_test test_detect_target_darwin
run_test test_latest_version_parses_release_tag_redirect
run_test test_latest_version_rejects_non_tag_redirect
run_test test_download_does_not_clobber_installer_dest
run_test test_checksum_failure
run_test test_checksum_success_host
run_test test_checksum_success_sha256sum_override
run_test test_checksum_success_shasum_override
run_test test_install_binary_darwin_postcopy
run_test test_setup_mode_interactive_stdin
run_test test_setup_mode_tty_fallback
run_test test_setup_mode_skip_env
run_test test_setup_mode_without_terminal
run_test test_run_setup_skips_when_tty_open_fails
run_test test_run_setup_fails_when_tty_setup_command_fails
run_test test_path_contains_dir_matches_exact_entry
run_test test_print_path_guidance_when_missing_after_setup
run_test test_print_path_guidance_is_silent_when_present
run_test test_main_installs_and_runs_setup
run_test test_main_runs_setup_via_tty_fallback
run_test test_main_skips_setup_when_requested
run_test test_main_fails_when_archive_layout_is_wrong
run_test test_main_skips_setup_without_terminal
run_test test_main_reports_path_guidance_after_successful_setup
run_test test_main_skips_setup_when_tty_open_fails
run_test test_main_fails_when_tty_setup_command_fails
