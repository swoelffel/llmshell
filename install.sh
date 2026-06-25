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
  url="$(curl -fsSL -o /dev/null -w '%{url_effective}' "https://github.com/$repo/releases/latest")"
  version="${url##*/}"
  [ -n "$version" ] || fail "unable to resolve latest release for $repo"
  printf '%s\n' "$version"
}

artifact_name() {
  version="$1"
  target="$2"
  printf 'llmsh-%s-%s.tar.gz\n' "$version" "$target"
}

download() {
  url="$1"
  dest="$2"
  curl -fsSL "$url" -o "$dest"
}

checksum_verify() {
  sums="$1"
  archive="$2"
  archive_dir="$(dirname -- "$archive")"
  archive_base="$(basename -- "$archive")"
  filtered="$(mktemp)"

  if ! awk -v name="$archive_base" '$2 == name { print; found=1 } END { exit(found ? 0 : 1) }' "$sums" > "$filtered"; then
    rm -f "$filtered"
    return 1
  fi

  case "$(uname -s)" in
    Linux)
      if (cd "$archive_dir" && sha256sum -c "$filtered" >/dev/null); then
        status=0
      else
        status=$?
      fi
      ;;
    Darwin)
      if (cd "$archive_dir" && shasum -a 256 -c "$filtered" >/dev/null); then
        status=0
      else
        status=$?
      fi
      ;;
    *)
      rm -f "$filtered"
      fail "unsupported operating system for checksum verification"
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
  if [ ! -f "$src" ]; then
    src="$(find "$extract_dir" -type f -name llmsh | head -n 1)"
  fi
  [ -n "$src" ] && [ -f "$src" ] || fail "llmsh binary not found in archive"

  install_binary "$src" "$dest"
  "$dest" --version

  if stdin_is_interactive && [ "${LLMSH_SKIP_SETUP:-0}" != "1" ]; then
    "$dest" setup
  fi
}

if [ "${LLMSH_INSTALL_TESTING:-0}" != "1" ]; then
  main "$@"
fi
