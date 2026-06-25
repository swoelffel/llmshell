#!/bin/sh
set -eu

fail() {
  printf 'install.sh: %s\n' "$*" >&2
  exit 1
}

detect_target() {
  os="$(uname -s)"
  arch="$(uname -m)"

  case "$arch" in
    x86_64|amd64) arch="x86_64" ;;
    arm64|aarch64) arch="aarch64" ;;
    *) fail "unsupported architecture: $arch" ;;
  esac

  case "$os" in
    Linux) printf '%s\n' "${arch}-unknown-linux-gnu" ;;
    Darwin) printf '%s\n' "${arch}-apple-darwin" ;;
    *) fail "unsupported operating system: $os" ;;
  esac
}

latest_version() {
  repo="$1"
  url="$(latest_release_effective_url "$repo")"
  case "$url" in
    "https://github.com/$repo/releases/tag/"*)
      version="${url##*/}"
      ;;
    *)
      fail "unable to resolve latest release for $repo from redirect: $url"
      ;;
  esac
  [ -n "$version" ] || fail "unable to resolve latest release for $repo"
  printf '%s\n' "$version"
}

latest_release_effective_url() {
  repo="$1"
  curl -fsSL -o /dev/null -w '%{url_effective}' "https://github.com/$repo/releases/latest"
}

artifact_name() {
  version="$1"
  target="$2"
  printf 'llmsh-%s-%s.tar.gz\n' "$version" "$target"
}

download() {
  url="$1"
  download_dest="$2"
  curl -fsSL "$url" -o "$download_dest"
}

checksum_tool() {
  if [ -n "${LLMSH_CHECKSUM_TOOL:-}" ]; then
    printf '%s\n' "$LLMSH_CHECKSUM_TOOL"
    return 0
  fi

  case "$(uname -s)" in
    Linux) printf 'sha256sum\n' ;;
    Darwin) printf 'shasum\n' ;;
    *) fail "unsupported operating system for checksum verification" ;;
  esac
}

checksum_verify() {
  sums="$1"
  archive="$2"
  archive_dir="$(dirname -- "$archive")"
  archive_base="$(basename -- "$archive")"
  filtered="$(mktemp)"
  tool="$(checksum_tool)"

  if ! awk -v name="$archive_base" '$2 == name { print; found=1 } END { exit(found ? 0 : 1) }' "$sums" > "$filtered"; then
    rm -f "$filtered"
    return 1
  fi

  case "$tool" in
    sha256sum)
      command -v sha256sum >/dev/null 2>&1 || fail "sha256sum is required for checksum verification"
      if (cd "$archive_dir" && sha256sum -c "$filtered" >/dev/null); then
        status=0
      else
        status=$?
      fi
      ;;
    shasum)
      command -v shasum >/dev/null 2>&1 || fail "shasum is required for checksum verification"
      if (cd "$archive_dir" && shasum -a 256 -c "$filtered" >/dev/null); then
        status=0
      else
        status=$?
      fi
      ;;
    *)
      rm -f "$filtered"
      fail "unsupported checksum tool: $tool"
      ;;
  esac

  rm -f "$filtered"
  return "${status:-0}"
}

install_binary() {
  src="$1"
  dest="$2"

  mkdir -p "$(dirname -- "$dest")"
  cp "$src" "$dest"
  chmod 0755 "$dest"

  if [ "$(uname -s)" = "Darwin" ]; then
    if command -v xattr >/dev/null 2>&1; then
      xattr -c "$dest"
    fi
    if command -v codesign >/dev/null 2>&1; then
      codesign --force --sign - "$dest"
    fi
  fi
}

stdin_is_interactive() {
  [ -t 0 ]
}

tty_device() {
  printf '%s\n' "/dev/tty"
}

tty_is_available() {
  tty="$(tty_device)"
  ( : < "$tty" > "$tty" ) >/dev/null 2>&1
}

tty_can_open() {
  tty="$1"
  ( : < "$tty" > "$tty" ) >/dev/null 2>&1
}

print_setup_skip_message() {
  reason="$1"
  printf '%s\n' "llmsh installer: $reason Run 'llmsh setup' or 'llmsh' later from a terminal." >&2
}

setup_mode() {
  if [ "${LLMSH_SKIP_SETUP:-0}" = "1" ]; then
    printf '%s\n' "skip"
  elif stdin_is_interactive; then
    printf '%s\n' "stdin"
  elif tty_is_available; then
    printf '%s\n' "tty"
  else
    printf '%s\n' "none"
  fi
}

run_setup() {
  dest="$1"
  RUN_SETUP_RESULT="skip"

  case "$(setup_mode)" in
    skip)
      return 0
      ;;
    stdin)
      RUN_SETUP_RESULT="stdin"
      "$dest" setup
      ;;
    tty)
      tty="$(tty_device)"
      if ! tty_can_open "$tty"; then
        RUN_SETUP_RESULT="none"
        print_setup_skip_message "unable to open $tty for interactive setup; skipping setup."
        return 0
      fi
      RUN_SETUP_RESULT="tty"
      "$dest" setup < "$tty" > "$tty"
      ;;
    none)
      RUN_SETUP_RESULT="none"
      print_setup_skip_message "no interactive terminal detected; skipping setup."
      ;;
    *)
      fail "unexpected setup mode"
      ;;
  esac
}

path_contains_dir() {
  dir="$1"
  case ":${PATH:-}:" in
    *":$dir:"*) return 0 ;;
    *) return 1 ;;
  esac
}

print_path_guidance() {
  install_dir="$1"
  dest="$2"
  mode="$3"

  if path_contains_dir "$install_dir"; then
    return 0
  fi

  printf '%s\n' "llmsh installer: $install_dir is not on your PATH." >&2
  printf '%s\n' "Add it in your shell before running 'llmsh' by name:" >&2
  printf '  export PATH="%s:$PATH"\n' "$install_dir" >&2

  case "$mode" in
    stdin|tty)
      printf '%s\n' "Setup already ran via '$dest setup'." >&2
      ;;
    *)
      :
      ;;
  esac
}

main() {
  repo="${LLMSH_REPO:-swoelffel/llmshell}"
  version="${LLMSH_VERSION:-$(latest_version "$repo")}"

  case "$version" in
    v*) ;;
    *) version="v$version" ;;
  esac

  target="$(detect_target)"
  artifact="$(artifact_name "$version" "$target")"
  base_url="https://github.com/$repo/releases/download/$version"
  tmpdir="$(mktemp -d)"
  archive="$tmpdir/$artifact"
  sums="$tmpdir/SHA256SUMS"
  extract_dir="$tmpdir/extract"
  dest_dir="${LLMSH_INSTALL_DIR:-$HOME/.local/bin}"
  dest="$dest_dir/llmsh"
  src=""

  cleanup() {
    rm -rf "$tmpdir"
  }
  trap cleanup EXIT INT TERM HUP

  download "$base_url/$artifact" "$archive"
  download "$base_url/SHA256SUMS" "$sums"
  checksum_verify "$sums" "$archive"

  mkdir -p "$extract_dir"
  tar -xzf "$archive" -C "$extract_dir"

  src="$extract_dir/llmsh-$version-$target/llmsh"
  [ -f "$src" ] || fail "expected release layout llmsh-$version-$target/llmsh inside $artifact"

  install_binary "$src" "$dest"
  "$dest" --version

  run_setup "$dest" || return $?
  print_path_guidance "$dest_dir" "$dest" "${RUN_SETUP_RESULT:-skip}"
}

if [ "${LLMSH_INSTALL_TESTING:-0}" != "1" ]; then
  main "$@"
fi
