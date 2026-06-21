#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Install smol-workflows binaries.

Usage:
  ./install.sh [options]

Install sources:
  --from-source       Build from this source checkout and install target binaries.
  --from-release      Download official GitHub release archive and install binaries.
  --version <tag>     Release tag for --from-release, for example v0.3.0-alpha.4.
                      Defaults to latest release.

If neither --from-source nor --from-release is set, the installer downloads the
latest official GitHub release. Use --from-source when you explicitly want to
build and install from a local checkout.

Options:
  --dir <path>        Install directory. Defaults to $INSTALL_DIR, $BIN_DIR, or ~/.local/bin.
  --debug             Source mode only: build/install debug-profile binaries instead of release.
  --no-build          Source mode only: do not run cargo build; copy existing target binaries.
  --no-locked         Source mode only: do not pass --locked to cargo build.
  -h, --help          Show this help.

Environment:
  INSTALL_DIR         Install destination override. Takes precedence over BIN_DIR.
  BIN_DIR             Install destination override when INSTALL_DIR is unset.
  PROFILE             Source mode cargo profile: release or debug. Default: release.
  NO_BUILD=1          Same as --no-build.
  CARGO               Cargo executable. Default: cargo.
  CARGO_FLAGS         Source mode: extra flags appended to cargo build.
  INSTALL_LOCKED=0    Same as --no-locked.
  BINS                Space-separated binary names to install.
                      Default: "smol-wf smol-sandbox-exe-dev".
  RELEASE_VERSION     Release tag for release mode, for example v0.3.0-alpha.4.
  RELEASE_BASE        Release download base URL. Defaults to latest GitHub release.
  RELEASE_REPO        GitHub repo for versioned releases. Default: b4fun/smol-workflows.
  DOWNLOAD_URL        Full archive URL override for release mode.

Examples:
  ./install.sh
  ./install.sh --dir ~/.local/bin
  INSTALL_DIR=/usr/local/bin ./install.sh

  ./install.sh --version v0.3.0-alpha.4
  ./install.sh --from-source
  BIN_DIR="$PWD/.tmp/bin" ./install.sh --from-source --debug
  curl -fsSL https://raw.githubusercontent.com/b4fun/smol-workflows/main/install.sh | bash
  curl -fsSL https://raw.githubusercontent.com/b4fun/smol-workflows/main/install.sh | bash -s -- --dir ~/.local/bin

Installed binaries by default:
  - smol-wf
  - smol-sandbox-exe-dev
EOF
}

fail() {
  echo "install.sh: $*" >&2
  exit 1
}

script_dir() {
  local source=${BASH_SOURCE[0]}
  if [ -z "$source" ] || [ "$source" = "bash" ] || [ ! -e "$source" ]; then
    pwd
    return
  fi
  while [ -L "$source" ]; do
    local dir
    dir=$(cd -P "$(dirname "$source")" >/dev/null 2>&1 && pwd)
    source=$(readlink "$source")
    [[ $source != /* ]] && source="$dir/$source"
  done
  cd -P "$(dirname "$source")" >/dev/null 2>&1 && pwd
}

archive_for_platform() {
  local os arch
  os=$(uname -s)
  arch=$(uname -m)
  case "$os:$arch" in
    Linux:x86_64|Linux:amd64) echo "smol-wf-linux-x86_64.tar.gz" ;;
    Darwin:arm64|Darwin:aarch64) echo "smol-wf-macos-aarch64.tar.gz" ;;
    MINGW*:x86_64|MSYS*:x86_64|CYGWIN*:x86_64) echo "smol-wf-windows-x86_64.zip" ;;
    *) fail "unsupported platform for official release download: $os $arch. Use --from-source or set DOWNLOAD_URL." ;;
  esac
}

release_url_for_archive() {
  local archive=$1
  if [ -n "${DOWNLOAD_URL:-}" ]; then
    echo "$DOWNLOAD_URL"
    return
  fi
  if [ -n "${RELEASE_BASE:-}" ]; then
    echo "${RELEASE_BASE%/}/$archive"
    return
  fi
  if [ -n "${RELEASE_VERSION:-}" ]; then
    echo "https://github.com/${RELEASE_REPO:-b4fun/smol-workflows}/releases/download/${RELEASE_VERSION}/$archive"
    return
  fi
  echo "https://github.com/${RELEASE_REPO:-b4fun/smol-workflows}/releases/latest/download/$archive"
}

extract_archive() {
  local archive_path=$1 dest_dir=$2
  case "$archive_path" in
    *.tar.gz|*.tgz)
      tar -xzf "$archive_path" -C "$dest_dir"
      ;;
    *.zip)
      command -v unzip >/dev/null 2>&1 || fail "unzip is required to extract $archive_path"
      unzip -q "$archive_path" -d "$dest_dir"
      ;;
    *)
      fail "unsupported release archive format: $archive_path"
      ;;
  esac
}

install_dir=${INSTALL_DIR:-${BIN_DIR:-$HOME/.local/bin}}
profile=${PROFILE:-release}
no_build=${NO_BUILD:-0}
install_locked=${INSTALL_LOCKED:-1}
cargo_bin=${CARGO:-cargo}
bins=${BINS:-"smol-wf smol-sandbox-exe-dev"}
mode=${INSTALL_SOURCE:-release}

while [ $# -gt 0 ]; do
  case "$1" in
    --from-source)
      mode=source
      shift
      ;;
    --from-release)
      mode=release
      shift
      ;;
    --version)
      if [ $# -lt 2 ]; then
        fail "--version requires a release tag"
      fi
      RELEASE_VERSION=$2
      mode=release
      shift 2
      ;;
    --version=*)
      RELEASE_VERSION=${1#--version=}
      mode=release
      shift
      ;;
    --dir)
      if [ $# -lt 2 ]; then
        fail "--dir requires a path"
      fi
      install_dir=$2
      shift 2
      ;;
    --dir=*)
      install_dir=${1#--dir=}
      shift
      ;;
    --debug)
      profile=debug
      shift
      ;;
    --no-build)
      no_build=1
      shift
      ;;
    --no-locked)
      install_locked=0
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "install.sh: unknown option: $1" >&2
      echo "Try './install.sh --help'." >&2
      exit 2
      ;;
  esac
done

case "$profile" in
  release|debug) ;;
  *) fail "unsupported PROFILE '$profile' (expected release or debug)" ;;
esac

repo_root=$(script_dir)

mkdir -p "$install_dir"

case "$mode" in
  source)
    cd "$repo_root"
    [ -f Cargo.toml ] || fail "Cargo.toml not found at $repo_root; use --from-release to install official release binaries"

    if [ "$no_build" != "1" ]; then
      build_args=(build)
      if [ "$profile" = "release" ]; then
        build_args+=(--release)
      fi
      if [ "$install_locked" != "0" ]; then
        build_args+=(--locked)
      fi
      build_args+=(
        -p smol-workflow-cli
        -p smol-sandbox-exe-dev
      )
      if [ -n "${CARGO_FLAGS:-}" ]; then
        # shellcheck disable=SC2206
        extra_flags=(${CARGO_FLAGS})
        build_args+=("${extra_flags[@]}")
      fi

      echo "==> Building smol-workflows binaries ($profile)"
      "$cargo_bin" "${build_args[@]}"
    else
      echo "==> Skipping build (--no-build)"
    fi

    target_dir="target/$profile"
    echo "==> Installing source-built binaries to $install_dir"
    for bin in $bins; do
      src="$target_dir/$bin"
      [ -f "$src" ] || fail "expected binary not found: $src. Run without --no-build, or set PROFILE/BINS appropriately."
      install -m 0755 "$src" "$install_dir/$bin"
      echo "  installed $install_dir/$bin"
    done
    ;;

  release)
    archive=$(archive_for_platform)
    url=$(release_url_for_archive "$archive")
    tmp_dir=$(mktemp -d)
    trap 'rm -rf "$tmp_dir"' EXIT
    archive_path="$tmp_dir/$archive"

    command -v curl >/dev/null 2>&1 || fail "curl is required for --from-release"
    echo "==> Downloading $url"
    curl -fL --retry 3 --connect-timeout 15 -o "$archive_path" "$url"

    echo "==> Extracting $archive"
    mkdir -p "$tmp_dir/extract"
    extract_archive "$archive_path" "$tmp_dir/extract"

    echo "==> Installing release binaries to $install_dir"
    for bin in $bins; do
      src="$tmp_dir/extract/$bin"
      if [ ! -f "$src" ] && [ -f "$tmp_dir/extract/$bin.exe" ]; then
        src="$tmp_dir/extract/$bin.exe"
      fi
      if [ ! -f "$src" ]; then
        available=$(find "$tmp_dir/extract" -maxdepth 2 -type f -exec basename {} \; 2>/dev/null | tr '\n' ' ')
        fail "release archive $archive does not contain requested binary '$bin'. Available files: $available"
      fi
      install -m 0755 "$src" "$install_dir/$(basename "$src")"
      echo "  installed $install_dir/$(basename "$src")"
    done
    ;;

  *)
    fail "unsupported install source '$mode' (expected auto, source, or release)"
    ;;
esac

case ":${PATH}:" in
  *":${install_dir}:"*) ;;
  *)
    cat <<EOF

Note: $install_dir is not currently on PATH.
Add this to your shell profile if needed:

  export PATH="$install_dir:\$PATH"
EOF
    ;;
esac

cat <<EOF

Done. Installed binaries in $install_dir:
$(find "$install_dir" -maxdepth 1 -type f \( -name 'smol-wf*' -o -name 'smol-sandbox-exe-dev*' \) -print | sort | sed 's/^/  /')
EOF
