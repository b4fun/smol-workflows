#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  smol-wf.sh list
  smol-wf.sh run <workflow-script> <args-file> <token-budget> [-- <extra smol-wf run flags...>]
  smol-wf.sh <workflow-script> <args-file> <token-budget> [-- <extra smol-wf run flags...>]

Commands:
  list  Prepare smol-wf and run: smol-wf llm list-workflows
  run   Prepare smol-wf and run a workflow script

Parameters for run:
  <workflow-script>  Path to the workflow .js/.mjs file.
  <args-file>        Path to a JSON object file for --args-from-file.
  <token-budget>     Output-token budget for --budget-allowance. Use 0, none, or - to omit.

Environment:
  SMOL_WF_BIN                 Explicit smol-wf binary path.
  SMOL_WF_AGENT_PROVIDER      Optional provider passed as --agent-provider.
  SMOL_WF_MAX_PARALLEL_AGENTS Concurrency cap. Defaults to 4.
  SMOL_WF_INSTALL_DIR         Download/install dir. Defaults to ~/.cache/smol-workflows/bin.
  SMOL_WF_RELEASE_BASE        Release base URL. Defaults to https://github.com/b4fun/smol-workflows/releases/latest/download.
  SMOL_WF_VERSION             Release tag, e.g. v0.1.0. Overrides latest URL form.
  SMOL_WF_REPO                GitHub repo for versioned downloads. Defaults to b4fun/smol-workflows.

Resolution order:
  1. SMOL_WF_BIN
  2. smol-wf on PATH
  3. target/release or target/debug under nearest smol-workflows Cargo workspace
  4. cargo build --release in nearest smol-workflows Cargo workspace, if cargo exists
  5. cached/downloaded binary from GitHub releases
EOF
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  usage
  exit 0
fi

fail() {
  echo "smol-wf helper: error: $*" >&2
  exit 1
}

log() {
  echo "[prepare-smol-wf] $*" >&2
}

is_executable_file() {
  [[ -n "${1:-}" && -f "$1" && -x "$1" ]]
}

find_workspace_root() {
  local dir="$PWD"
  while [[ "$dir" != "/" ]]; do
    if [[ -f "$dir/Cargo.toml" ]] && grep -q 'smol-workflow-cli\|rust/cli' "$dir/Cargo.toml" 2>/dev/null; then
      printf '%s\n' "$dir"
      return 0
    fi
    dir="$(dirname "$dir")"
  done
  return 1
}

platform_archive() {
  local os arch
  os="$(uname -s)"
  arch="$(uname -m)"
  case "$os:$arch" in
    Darwin:arm64|Darwin:aarch64) echo "smol-wf-macos-aarch64.tar.gz" ;;
    Linux:x86_64|Linux:amd64) echo "smol-wf-linux-x86_64.tar.gz" ;;
    MINGW*:x86_64|MSYS*:x86_64|CYGWIN*:x86_64) echo "smol-wf-windows-x86_64.zip" ;;
    *) fail "unsupported platform for binary download: $os $arch. Install smol-wf or set SMOL_WF_BIN." ;;
  esac
}

extract_binary() {
  local archive_path="$1" dest_dir="$2"
  case "$archive_path" in
    *.tar.gz) tar -xzf "$archive_path" -C "$dest_dir" ;;
    *.zip)
      command -v unzip >/dev/null 2>&1 || fail "unzip is required to extract $archive_path"
      unzip -o "$archive_path" -d "$dest_dir" >/dev/null
      ;;
    *) fail "unknown archive type: $archive_path" ;;
  esac
}

download_smol_wf() {
  local install_dir archive url tmp_archive bin_name repo
  install_dir="${SMOL_WF_INSTALL_DIR:-$HOME/.cache/smol-workflows/bin}"
  mkdir -p "$install_dir"
  archive="$(platform_archive)"
  bin_name="smol-wf"
  [[ "$archive" == *.zip ]] && bin_name="smol-wf.exe"

  if is_executable_file "$install_dir/$bin_name"; then
    printf '%s\n' "$install_dir/$bin_name"
    return 0
  fi

  if [[ -n "${SMOL_WF_VERSION:-}" ]]; then
    repo="${SMOL_WF_REPO:-b4fun/smol-workflows}"
    url="https://github.com/${repo}/releases/download/${SMOL_WF_VERSION}/${archive}"
  else
    url="${SMOL_WF_RELEASE_BASE:-https://github.com/b4fun/smol-workflows/releases/latest/download}/${archive}"
  fi

  command -v curl >/dev/null 2>&1 || fail "curl is required to download smol-wf; install smol-wf or set SMOL_WF_BIN"
  tmp_archive="$(mktemp -t "${archive}.XXXXXX")"
  log "downloading $url"
  if ! curl -fL --retry 2 -o "$tmp_archive" "$url"; then
    rm -f "$tmp_archive"
    fail "failed to download smol-wf from $url. Install it manually, set SMOL_WF_BIN, or set SMOL_WF_RELEASE_BASE/SMOL_WF_VERSION."
  fi
  extract_binary "$tmp_archive" "$install_dir"
  rm -f "$tmp_archive"
  chmod +x "$install_dir/$bin_name" 2>/dev/null || true
  is_executable_file "$install_dir/$bin_name" || fail "downloaded archive did not produce executable $install_dir/$bin_name"
  printf '%s\n' "$install_dir/$bin_name"
}

resolve_smol_wf() {
  local workspace candidate
  if [[ -n "${SMOL_WF_BIN:-}" ]]; then
    is_executable_file "$SMOL_WF_BIN" || fail "SMOL_WF_BIN is not executable: $SMOL_WF_BIN"
    printf '%s\n' "$SMOL_WF_BIN"
    return 0
  fi

  if command -v smol-wf >/dev/null 2>&1; then
    command -v smol-wf
    return 0
  fi

  if workspace="$(find_workspace_root)"; then
    for candidate in \
      "$workspace/target/release/smol-wf" \
      "$workspace/target/debug/smol-wf" \
      "$workspace/target/release/smol-wf.exe" \
      "$workspace/target/debug/smol-wf.exe"; do
      if is_executable_file "$candidate"; then
        printf '%s\n' "$candidate"
        return 0
      fi
    done

    if command -v cargo >/dev/null 2>&1; then
      log "building smol-wf with cargo in $workspace"
      (cd "$workspace" && cargo build --release --locked -p smol-workflow-cli)
      for candidate in "$workspace/target/release/smol-wf" "$workspace/target/release/smol-wf.exe"; do
        if is_executable_file "$candidate"; then
          printf '%s\n' "$candidate"
          return 0
        fi
      done
    fi
  fi

  download_smol_wf
}

validate_args_file() {
  local args_file="$1"
  [[ -f "$args_file" ]] || fail "args file does not exist: $args_file"
  if command -v jq >/dev/null 2>&1; then
    jq -e 'type == "object"' "$args_file" >/dev/null || fail "args file must contain a JSON object: $args_file"
  elif command -v node >/dev/null 2>&1; then
    node -e 'const fs=require("fs"); const v=JSON.parse(fs.readFileSync(process.argv[1],"utf8")); if (!v || Array.isArray(v) || typeof v !== "object") process.exit(1)' "$args_file" \
      || fail "args file must contain a JSON object: $args_file"
  else
    log "warning: jq/node not found; skipping JSON object validation for $args_file"
  fi
}

run_list() {
  if [[ $# -ne 0 ]]; then
    fail "list does not accept arguments"
  fi
  local smol_wf
  smol_wf="$(resolve_smol_wf)"
  log "using smol-wf: $smol_wf"
  exec "$smol_wf" llm list-workflows
}

run_workflow() {
  if [[ $# -lt 3 ]]; then
    usage >&2
    exit 2
  fi

  local workflow_script="$1"
  local args_file="$2"
  local token_budget="$3"
  shift 3
  if [[ "${1:-}" == "--" ]]; then
    shift
  fi
  local extra_flags=("$@")

  [[ -f "$workflow_script" ]] || fail "workflow script does not exist: $workflow_script"
  validate_args_file "$args_file"

  local smol_wf
  smol_wf="$(resolve_smol_wf)"
  log "using smol-wf: $smol_wf"

  local cmd=("$smol_wf" run "$workflow_script" --args-from-file "$args_file")
  if [[ "$token_budget" != "0" && "$token_budget" != "none" && "$token_budget" != "-" ]]; then
    cmd+=(--budget-allowance "$token_budget")
  fi
  if [[ -n "${SMOL_WF_AGENT_PROVIDER:-}" ]]; then
    cmd+=(--agent-provider "$SMOL_WF_AGENT_PROVIDER")
  fi
  cmd+=(--max-parallel-agents "${SMOL_WF_MAX_PARALLEL_AGENTS:-4}")
  cmd+=("${extra_flags[@]}")

  log "executing: ${cmd[*]}"
  exec "${cmd[@]}"
}

case "${1:-}" in
  list)
    shift
    run_list "$@"
    ;;
  run)
    shift
    run_workflow "$@"
    ;;
  "")
    usage >&2
    exit 2
    ;;
  *)
    run_workflow "$@"
    ;;
esac
