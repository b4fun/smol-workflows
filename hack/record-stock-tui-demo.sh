#!/usr/bin/env bash
# Record a smol-wf live TUI demo for the stock workflow.
#
# What this script does:
#   1. Builds smol-wf.
#   2. Creates an isolated temporary workspace.
#   3. Copies the smol-wf binary and examples/stock.mjs into that workspace.
#   4. Uses VHS to record a live TUI run against the selected provider.
#   5. Speeds the recording up to a 30s MP4 and GIF.
#   6. Copies the 30s outputs to docs/assets/.
#
# Requirements:
#   - nix-shell must be available. The script uses:
#       nix-shell -p vhs ffmpeg
#
# Usage:
#   hack/record-stock-tui-demo.sh
#
# Useful environment variables:
#   SMOL_WF_PROVIDER=pi|debug     Agent provider to use. Defaults to pi.
#   SMOL_WF_STOCK=NVDA            Stock symbol to pass to stock.mjs. Defaults to NVDA.
#   SMOL_WF_VHS_WIDTH=1280        VHS terminal width in pixels.
#   SMOL_WF_VHS_HEIGHT=760        VHS terminal height in pixels.
#   SMOL_WF_RESEARCH_WAIT=240s    Max wait for Research phase.
#   SMOL_WF_DONE_WAIT=300s        Max wait for live run completion.
#   SMOL_WF_SAVE_WAIT=60s         Max wait for save prompt / saved message.
#   SMOL_WF_DEMO_WORKSPACE=path   Reuse/write a specific workspace instead of mktemp.
#
# Outputs:
#   docs/assets/smol-wf-tui-stock-demo-30s.mp4
#   docs/assets/smol-wf-tui-stock-demo-30s.gif
#
# The temporary workspace path is printed at the end and contains the full-length
# VHS outputs, the event JSONL saved by the TUI, workflows.db, and the generated
# stock-live.tape for debugging/re-recording.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORKSPACE="${SMOL_WF_DEMO_WORKSPACE:-$(mktemp -d "${TMPDIR:-/tmp}/smol-wf-stock-vhs-only.XXXXXX")}" 
PROVIDER="${SMOL_WF_PROVIDER:-pi}"
STOCK="${SMOL_WF_STOCK:-NVDA}"
WIDTH="${SMOL_WF_VHS_WIDTH:-1280}"
HEIGHT="${SMOL_WF_VHS_HEIGHT:-760}"
RESEARCH_WAIT="${SMOL_WF_RESEARCH_WAIT:-240s}"
DONE_WAIT="${SMOL_WF_DONE_WAIT:-300s}"
SAVE_WAIT="${SMOL_WF_SAVE_WAIT:-60s}"
ASSET_BASENAME="smol-wf-tui-stock-demo-30s"

mkdir -p "$WORKSPACE" "$ROOT/docs/assets"

cd "$ROOT"
cargo build -q -p smol-workflow-cli
cp "$ROOT/target/debug/smol-wf" "$WORKSPACE/smol-wf"
cp "$ROOT/examples/stock.mjs" "$WORKSPACE/stock.mjs"
cat > "$WORKSPACE/args.json" <<JSON
{"stocks":["$STOCK"]}
JSON

cat > "$WORKSPACE/stock-live.tape" <<EOF
Output stock-live.gif
Output stock-live.mp4
Set Shell "bash"
Set FontSize 14
Set Width $WIDTH
Set Height $HEIGHT
Set TypingSpeed 10ms
Set Framerate 24

# Clear shell startup messages such as local mail notifications before showing
# the demo command.
Hide
Type "clear"
Enter
Show

Type "./smol-wf tui run ./stock.mjs --agent-provider $PROVIDER --db workflows.db --args-from-file args.json"
Enter

Wait+Screen@60s /LIVE RUNNING|LIVE DONE|LIVE FAILED/
Wait+Screen@$RESEARCH_WAIT /workflow.phase Research|Spawning [0-9]+ research agents/

# Move through timeline during Research.
Type "1"
Down@350ms 6
Up@350ms 2
Down@350ms 3

# Move selection back to the newest timeline item so follow-latest re-engages
# and newer events scroll naturally while the workflow continues.
Down@20ms 120
Sleep 2s

# Switch to details pane and scroll, then back to timeline.
Type "2"
Down@300ms 4
Up@300ms 2
Type "1"
Sleep 1s

# Wait for completion, then show final workflow log output.
Wait+Screen@$DONE_WAIT /LIVE DONE|LIVE FAILED/
Type "/"
Type@80ms "workflow.log"
Enter
Down@30ms 80
Sleep 1s
Type "m"
Sleep 2s
Type "m"
Sleep 4s

# Show save confirmation prompt, then activate the default save & quit button.
Type "q"
Wait+Screen@$SAVE_WAIT /save & quit|Quit without saving event log|confirm quit/
Sleep 4s
Enter
Wait+Screen@$SAVE_WAIT /Events log saved to/
Sleep 2s
EOF

cd "$WORKSPACE"
nix-shell -p vhs ffmpeg --run 'vhs stock-live.tape'

nix-shell -p ffmpeg --run '
set -euo pipefail
duration=$(ffprobe -v error -show_entries format=duration -of default=nk=1:nw=1 stock-live.mp4)
setpts=$(awk -v d="$duration" "BEGIN { printf \"%.8f\", 30.0/d }")
echo "duration=$duration setpts=$setpts"
ffmpeg -y -i stock-live.mp4 -filter:v "setpts=${setpts}*PTS" -an -movflags +faststart stock-live-30s.mp4
ffmpeg -y -i stock-live-30s.mp4 -vf "fps=12,scale=960:-1:flags=lanczos,palettegen=stats_mode=diff" stock-live-30s.palette.png
ffmpeg -y -i stock-live-30s.mp4 -i stock-live-30s.palette.png -lavfi "fps=12,scale=960:-1:flags=lanczos[x];[x][1:v]paletteuse=dither=bayer:bayer_scale=5:diff_mode=rectangle" stock-live-30s.gif
'

cp "$WORKSPACE/stock-live-30s.mp4" "$ROOT/docs/assets/$ASSET_BASENAME.mp4"
cp "$WORKSPACE/stock-live-30s.gif" "$ROOT/docs/assets/$ASSET_BASENAME.gif"

cat <<EOF
Recorded stock TUI demo.
Workspace: $WORKSPACE
MP4: $ROOT/docs/assets/$ASSET_BASENAME.mp4
GIF: $ROOT/docs/assets/$ASSET_BASENAME.gif
EOF
