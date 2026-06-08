#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source_root="$repo_root/harness/plugins/smol-workflows/skills"
target_root="$repo_root/rust/cli/assets/skills"

assets=(
  "create/SKILL.md"
  "list/SKILL.md"
  "run/SKILL.md"
  "scripts/smol-wf.sh"
)

mkdir -p \
  "$target_root/create" \
  "$target_root/list" \
  "$target_root/run" \
  "$target_root/scripts"

for asset in "${assets[@]}"; do
  cp "$source_root/$asset" "$target_root/$asset"
done

chmod +x "$target_root/scripts/smol-wf.sh"

echo "Synced ${#assets[@]} CLI skill assets from harness/plugins/smol-workflows/skills to rust/cli/assets/skills"
