import { exec as sandboxExec } from "workflow:sandbox";

export const meta = {
  name: 'rust-release-install-verify',
  description: 'Verify the latest smol-workflows GitHub release installs correctly from inside an exe.dev VM sandbox.',
  phases: [
    { title: 'release-install-verify', detail: 'Run inside an exe.dev VM sandbox, install from the latest release artifact, and verify smol-wf version', model: 'fast' },
  ],
}

const input = args && typeof args === 'object' ? args : {}
const expectedTag = typeof input.expectedTag === 'string' ? input.expectedTag.trim() : ''
const version = typeof input.version === 'string' ? input.version.trim() : ''
const remoteReleaseStatus = input.remoteReleaseStatus && typeof input.remoteReleaseStatus === 'object'
  ? input.remoteReleaseStatus
  : null

function shellSingleQuote(value) {
  return `'${String(value).replaceAll("'", "'\\''")}'`
}

function parseKeyValueBlock(stdout) {
  const start = stdout.indexOf('__SMOL_WF_VERIFY_BEGIN__')
  const end = stdout.indexOf('__SMOL_WF_VERIFY_END__')
  if (start < 0 || end < start) {
    throw new Error(`sandbox verification output did not contain parse markers. stdout:\n${stdout}`)
  }
  const block = stdout.slice(start, end).split('\n').slice(1)
  const values = {}
  for (const line of block) {
    if (!line.trim()) continue
    const eq = line.indexOf('=')
    if (eq < 0) continue
    values[line.slice(0, eq)] = line.slice(eq + 1)
  }
  return values
}

function decodeBase64(value) {
  if (!value) return ''
  const alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/='
  let output = ''
  let buffer = 0
  let bits = 0
  for (const char of value.replace(/\s/g, '')) {
    if (char === '=') break
    const index = alphabet.indexOf(char)
    if (index < 0) continue
    buffer = (buffer << 6) | index
    bits += 6
    if (bits >= 8) {
      bits -= 8
      output += String.fromCharCode((buffer >> bits) & 0xff)
    }
  }
  return output
}

phase('release-install-verify')
log(`Verifying latest release install artifact in an exe.dev sandbox for ${expectedTag || 'the latest GitHub Release'}`)

const script = String.raw`
set -eu
expected_tag_input=${shellSingleQuote(expectedTag)}
expected_version_input=${shellSingleQuote(version)}
install_root="$PWD/.tmp/release-install"
install_dir="$install_root/bin"
rm -rf "$install_root"
mkdir -p "$install_dir"

pwd_output=$(pwd)
uname_output=$(uname -a 2>/dev/null || true)
hostname_output=$(hostname 2>/dev/null || true)

latest_url=$(curl -fsSLI -o /dev/null -w '%{url_effective}' https://github.com/b4fun/smol-workflows/releases/latest)
latest_tag=$(basename "$latest_url")
if [ -z "$latest_tag" ] || [ "$latest_tag" = "latest" ]; then
  echo "failed to resolve latest release tag from $latest_url" >&2
  exit 20
fi

if [ -n "$expected_tag_input" ] && [ "$latest_tag" != "$expected_tag_input" ]; then
  echo "latest release tag mismatch: expected $expected_tag_input, got $latest_tag" >&2
  exit 21
fi

case "$latest_tag" in
  v*) derived_version=$(printf '%s' "$latest_tag" | sed 's/^v//') ;;
  *) echo "latest release tag is not v<semver>: $latest_tag" >&2; exit 22 ;;
esac

if [ -n "$expected_version_input" ]; then
  expected_version="$expected_version_input"
else
  expected_version="$derived_version"
fi

curl -fsSL https://raw.githubusercontent.com/b4fun/smol-workflows/main/install.sh -o "$install_root/install.sh"
install_status=0
install_output=$(INSTALL_DIR="$install_dir" bash "$install_root/install.sh" --from-release 2>&1) || install_status=$?
printf '%s\n' "$install_output"
if [ "$install_status" -ne 0 ]; then
  echo "install script failed with status $install_status" >&2
  exit "$install_status"
fi

smol_wf_path="$install_dir/smol-wf"
if [ ! -x "$smol_wf_path" ] && [ -x "$install_dir/smol-wf.exe" ]; then
  smol_wf_path="$install_dir/smol-wf.exe"
fi
if [ ! -x "$smol_wf_path" ]; then
  echo "installed smol-wf binary not found under $install_dir" >&2
  exit 23
fi

version_output=$("$smol_wf_path" --version 2>&1)
smol_wf_version=$(printf '%s\n' "$version_output" | sed -n 's/.*\([0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*[-+A-Za-z0-9.]*\).*/\1/p' | head -n 1)
if [ -z "$smol_wf_version" ]; then
  echo "failed to extract semver from version output: $version_output" >&2
  exit 24
fi
if [ "$smol_wf_version" != "$expected_version" ]; then
  echo "smol-wf version mismatch: expected $expected_version, got $smol_wf_version" >&2
  exit 25
fi
if [ "v$smol_wf_version" != "$latest_tag" ]; then
  echo "smol-wf version does not match latest tag: v$smol_wf_version vs $latest_tag" >&2
  exit 26
fi
case "$install_output" in
  *"/releases/latest/download/smol-wf-"*) ;;
  *) echo "installer output did not show a latest release artifact download URL" >&2; exit 27 ;;
esac

printf '__SMOL_WF_VERIFY_BEGIN__\n'
printf 'expected_tag=%s\n' "$latest_tag"
printf 'latest_release_tag=%s\n' "$latest_tag"
printf 'install_dir=%s\n' "$install_dir"
printf 'smol_wf_path=%s\n' "$smol_wf_path"
printf 'smol_wf_version=%s\n' "$smol_wf_version"
printf 'pwd_b64=%s\n' "$(printf '%s' "$pwd_output" | base64 | tr -d '\n')"
printf 'uname_b64=%s\n' "$(printf '%s' "$uname_output" | base64 | tr -d '\n')"
printf 'hostname_b64=%s\n' "$(printf '%s' "$hostname_output" | base64 | tr -d '\n')"
printf 'install_output_b64=%s\n' "$(printf '%s' "$install_output" | base64 | tr -d '\n')"
printf 'version_output_b64=%s\n' "$(printf '%s' "$version_output" | base64 | tr -d '\n')"
printf '__SMOL_WF_VERIFY_END__\n'
`

const installCommand = 'curl -fsSL https://raw.githubusercontent.com/b4fun/smol-workflows/main/install.sh | INSTALL_DIR="$PWD/.tmp/release-install/bin" bash -s -- --from-release'
const execResult = await sandboxExec('exe-dev/default', {
  command: 'sh',
  args: ['-lc', script],
})

if (execResult.exitCode !== 0) {
  throw new Error(`sandbox install verification command failed with exit code ${execResult.exitCode}\nstdout:\n${execResult.stdout}\nstderr:\n${execResult.stderr}`)
}

const values = parseKeyValueBlock(execResult.stdout)
const installOutput = decodeBase64(values.install_output_b64)
const versionOutput = decodeBase64(values.version_output_b64)
const pwdOutput = decodeBase64(values.pwd_b64)
const unameOutput = decodeBase64(values.uname_b64)
const hostnameOutput = decodeBase64(values.hostname_b64)

const installVerifyReport = {
  expectedTag: values.expected_tag,
  latestReleaseTag: values.latest_release_tag,
  installCommand,
  installDir: values.install_dir,
  smolWfPath: values.smol_wf_path,
  smolWfVersionOutput: versionOutput,
  smolWfVersion: values.smol_wf_version,
  versionMatchesLatestRelease: values.latest_release_tag === `v${values.smol_wf_version}`,
  sandboxVmVerified: true,
  summary: `Installed smol-wf ${values.smol_wf_version} from latest release ${values.latest_release_tag} inside exe.dev sandbox`,
  notes: [
    `pwd: ${pwdOutput}`,
    `uname: ${unameOutput}`,
    `hostname: ${hostnameOutput}`,
    `remoteReleaseStatus supplied: ${remoteReleaseStatus ? 'yes' : 'no'}`,
    `installer output: ${installOutput}`,
  ],
}

if (!installVerifyReport.sandboxVmVerified || !installVerifyReport.versionMatchesLatestRelease) {
  throw new Error(`Latest release install verification failed: ${installVerifyReport.summary}`)
}

log('Asking local agent to confirm captured sandbox install evidence')
const localAgentConfirmation = await agent(
  `Confirm whether this captured exe.dev sandbox command output verifies the latest smol-wf release install.

The command already ran inside the sandbox using workflow:sandbox exec. You are running locally now; do not create a sandbox and do not run install commands. Only inspect the evidence.

Structured report:
${JSON.stringify(installVerifyReport, null, 2)}

Sandbox command exit code: ${execResult.exitCode}
Sandbox command stderr:
${execResult.stderr}

Confirm only if:
- the sandbox command exit code is 0;
- the latest release tag is present;
- the installed smol-wf version matches the latest release tag;
- installer output shows a /releases/latest/download/smol-wf-* artifact URL;
- the evidence indicates commands ran in a sandbox/Linux environment.

Return a concise plain-text confirmation with any concerns.`,
  {
    phase: 'release-install-verify',
    label: 'local-confirm-sandbox-install-evidence',
  },
)

log(`Install verification working: ${installVerifyReport.summary}`)

export default {
  ...installVerifyReport,
  localAgentConfirmation,
}
