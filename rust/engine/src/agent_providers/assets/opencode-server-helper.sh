#!/usr/bin/env bash
# This helper runs inside the selected AgentExecutionEnvironment.
#
# OpenCode long-prompt and structured-output modes require starting an OpenCode
# HTTP server and then talking to its localhost API. For sandbox/remote
# environments, that localhost endpoint is only reachable from inside the same
# environment where the server is running, not from the host engine process.
#
# The Rust provider therefore writes this script plus JSON request files into the
# environment, executes it there, and reads the JSON response files back through
# environment file I/O. Keeping this helper as Bash avoids requiring a second
# Node helper runtime beyond whatever OpenCode itself needs.
set -euo pipefail

COMMAND=""
DIRECTORY=""
TIMEOUT_MS="15000"
SESSION_BODY=""
MESSAGE_BODY=""
SESSION_OUTPUT=""
RESPONSE_OUTPUT=""
LOGS_OUTPUT=""
SERVER_ARGS=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --command)
      COMMAND="$2"
      shift 2
      ;;
    --directory)
      DIRECTORY="$2"
      shift 2
      ;;
    --timeout-ms)
      TIMEOUT_MS="$2"
      shift 2
      ;;
    --session-body)
      SESSION_BODY="$2"
      shift 2
      ;;
    --message-body)
      MESSAGE_BODY="$2"
      shift 2
      ;;
    --session-output)
      SESSION_OUTPUT="$2"
      shift 2
      ;;
    --response-output)
      RESPONSE_OUTPUT="$2"
      shift 2
      ;;
    --logs-output)
      LOGS_OUTPUT="$2"
      shift 2
      ;;
    --)
      shift
      SERVER_ARGS=("$@")
      break
      ;;
    *)
      echo "unknown argument: $1" >&2
      exit 2
      ;;
  esac
done

if [[ -z "$COMMAND" || -z "$DIRECTORY" || -z "$SESSION_BODY" || -z "$MESSAGE_BODY" || -z "$SESSION_OUTPUT" || -z "$RESPONSE_OUTPUT" || -z "$LOGS_OUTPUT" ]]; then
  echo "missing required opencode helper arguments" >&2
  exit 2
fi

mkdir -p "$(dirname "$SESSION_OUTPUT")" "$(dirname "$RESPONSE_OUTPUT")" "$(dirname "$LOGS_OUTPUT")"
: >"$LOGS_OUTPUT"

SERVER_PID=""
cleanup() {
  if [[ -n "$SERVER_PID" ]] && kill -0 "$SERVER_PID" 2>/dev/null; then
    kill "$SERVER_PID" 2>/dev/null || true
    for _ in {1..20}; do
      if ! kill -0 "$SERVER_PID" 2>/dev/null; then
        wait "$SERVER_PID" 2>/dev/null || true
        return
      fi
      sleep 0.1
    done
    kill -9 "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
  fi
}
trap cleanup EXIT INT TERM

"$COMMAND" "${SERVER_ARGS[@]}" >"$LOGS_OUTPUT" 2>&1 &
SERVER_PID="$!"

TIMEOUT_SECS=$(( (TIMEOUT_MS + 999) / 1000 ))
if [[ "$TIMEOUT_SECS" -lt 1 ]]; then
  TIMEOUT_SECS=1
fi
START_SECONDS=$SECONDS
SERVER_URL=""
MARKER="opencode server listening on "

while true; do
  if ! kill -0 "$SERVER_PID" 2>/dev/null; then
    wait "$SERVER_PID" 2>/dev/null || true
    echo "OpenCode server exited before it was ready: $(tail -c 4000 "$LOGS_OUTPUT")" >&2
    exit 1
  fi

  SERVER_URL=$(awk -v marker="$MARKER" '
    index($0, marker) {
      rest = substr($0, index($0, marker) + length(marker));
      split(rest, parts, /[[:space:]]+/);
      print parts[1];
      exit;
    }
  ' "$LOGS_OUTPUT")
  if [[ -n "$SERVER_URL" ]]; then
    break
  fi

  if [[ $((SECONDS - START_SECONDS)) -ge "$TIMEOUT_SECS" ]]; then
    echo "Timed out waiting for OpenCode server URL: $(tail -c 4000 "$LOGS_OUTPUT")" >&2
    exit 1
  fi
  sleep 0.05
done

post_json() {
  local path="$1"
  local body_file="$2"
  local output_file="$3"
  local tmp_file="${output_file}.tmp"
  local http_code

  local request_url="${SERVER_URL}${path}?directory=${DIRECTORY// /%20}"
  http_code=$(curl -sS -o "$tmp_file" -w '%{http_code}' \
    -X POST -H 'content-type: application/json' \
    --data-binary "@$body_file" \
    "$request_url") || {
      local status=$?
      rm -f "$tmp_file"
      echo "OpenCode POST ${path} failed: curl exited with ${status}" >&2
      exit 1
    }

  if [[ "$http_code" -lt 200 || "$http_code" -ge 300 ]]; then
    local reason=""
    case "$http_code" in
      400) reason=" Bad Request" ;;
      401) reason=" Unauthorized" ;;
      403) reason=" Forbidden" ;;
      404) reason=" Not Found" ;;
      500) reason=" Internal Server Error" ;;
    esac
    echo "OpenCode POST ${path} failed with HTTP ${http_code}${reason}: $(cat "$tmp_file")" >&2
    rm -f "$tmp_file"
    exit 1
  fi

  mv "$tmp_file" "$output_file"
}

post_json "/session" "$SESSION_BODY" "$SESSION_OUTPUT"
SESSION_ID=$(sed -n 's/.*"id"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$SESSION_OUTPUT" | head -n 1)
if [[ -z "$SESSION_ID" ]]; then
  SESSION_ID=$(sed -n 's/.*"sessionID"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$SESSION_OUTPUT" | head -n 1)
fi
if [[ -z "$SESSION_ID" ]]; then
  SESSION_ID=$(sed -n 's/.*"session_id"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$SESSION_OUTPUT" | head -n 1)
fi
if [[ -z "$SESSION_ID" ]]; then
  echo "OpenCode create-session response did not include an id: $(cat "$SESSION_OUTPUT")" >&2
  exit 1
fi

post_json "/session/${SESSION_ID}/message" "$MESSAGE_BODY" "$RESPONSE_OUTPUT"
