import { sleep } from "workflow:extra";

export const meta = {
  name: 'rust-crate-publish',
  description: 'Prepare and finalize a Rust crates.io release from a natural-language request: resolve version, open PR, wait for merge, create the GitHub release tag, and verify the published install artifact',
  phases: [
    { title: 'prepare', detail: 'Detect release intent and choose the next release version', model: 'fast' },
    { title: 'release-prepare-code', detail: 'Update local Rust release files, validate, commit, push, and open a PR', model: 'smart' },
    { title: 'release-prepare-pr', detail: 'Poll the release PR until it is merged', model: 'fast' },
    { title: 'release-finalize', detail: 'Create the GitHub release/tag on the merged commit and wait for publish workflows', model: 'smart' },
    { title: 'release-install-verify', detail: 'Create an exe.dev VM sandbox, install from the latest release artifact, and verify smol-wf version', model: 'smart' },
  ],
}

const RELEASE_INTENT_SCHEMA = {
  type: 'object',
  properties: {
    intent: { type: 'string' },
    mode: { type: 'string', enum: ['explicit-version', 'semver-bump', 'alpha-bump'] },
    version: {
      type: 'string',
      description: 'Final chosen semver version without a leading v.',
    },
    expectedTag: { type: 'string', description: 'Expected release tag, for example v0.2.1.' },
    currentVersion: { type: 'string' },
    bump: { type: 'string', enum: ['none', 'patch', 'minor', 'major', 'alpha'] },
    summary: { type: 'string' },
    assumptions: { type: 'array', items: { type: 'string' } },
    verification: { type: 'array', items: { type: 'string' } },
  },
  required: ['intent', 'mode', 'version', 'expectedTag', 'currentVersion', 'bump', 'summary', 'assumptions', 'verification'],
}

const CODE_PREP_REPORT_SCHEMA = {
  type: 'object',
  properties: {
    release: { type: 'string' },
    expectedTag: { type: 'string' },
    branch: { type: 'string' },
    commit: { type: 'string' },
    prUrl: { type: 'string' },
    prNumber: { type: 'integer' },
    filesChanged: { type: 'array', items: { type: 'string' } },
    validation: { type: 'array', items: { type: 'string' } },
    notes: { type: 'array', items: { type: 'string' } },
  },
  required: ['release', 'expectedTag', 'branch', 'commit', 'prUrl', 'prNumber', 'filesChanged', 'validation', 'notes'],
}

const PR_STATUS_SCHEMA = {
  type: 'object',
  properties: {
    prUrl: { type: 'string' },
    prNumber: { type: 'integer' },
    state: { type: 'string' },
    merged: { type: 'boolean' },
    mergeCommit: { type: 'string' },
    baseBranch: { type: 'string' },
    headBranch: { type: 'string' },
    summary: { type: 'string' },
    notes: { type: 'array', items: { type: 'string' } },
  },
  required: ['prUrl', 'prNumber', 'state', 'merged', 'mergeCommit', 'baseBranch', 'headBranch', 'summary', 'notes'],
}

const FINAL_REPORT_SCHEMA = {
  type: 'object',
  properties: {
    release: { type: 'string' },
    expectedTag: { type: 'string' },
    releaseUrl: { type: 'string' },
    targetCommit: { type: 'string' },
    cratesPublishWorkflow: { type: 'string' },
    binaryReleaseWorkflow: { type: 'string' },
    validation: { type: 'array', items: { type: 'string' } },
    notes: { type: 'array', items: { type: 'string' } },
  },
  required: ['release', 'expectedTag', 'releaseUrl', 'targetCommit', 'cratesPublishWorkflow', 'binaryReleaseWorkflow', 'validation', 'notes'],
}

const REMOTE_RELEASE_STATUS_SCHEMA = {
  type: 'object',
  properties: {
    expectedTag: { type: 'string' },
    releaseUrl: { type: 'string' },
    targetCommit: { type: 'string' },
    allRunsStarted: { type: 'boolean' },
    allRunsCompleted: { type: 'boolean' },
    allRunsSuccessful: { type: 'boolean' },
    workflows: {
      type: 'array',
      items: {
        type: 'object',
        properties: {
          workflow: { type: 'string' },
          runUrl: { type: 'string' },
          status: { type: 'string' },
          conclusion: { type: 'string' },
        },
        required: ['workflow', 'runUrl', 'status', 'conclusion'],
      },
    },
    summary: { type: 'string' },
    notes: { type: 'array', items: { type: 'string' } },
  },
  required: ['expectedTag', 'releaseUrl', 'targetCommit', 'allRunsStarted', 'allRunsCompleted', 'allRunsSuccessful', 'workflows', 'summary', 'notes'],
}

const rawReleaseInput = args && typeof args === 'object' ? args : {}
const releaseRequest = typeof rawReleaseInput.intent === 'string'
  ? rawReleaseInput.intent.trim()
  : ''

if (!releaseRequest) {
  throw new Error('Missing natural-language release intent. Pass an args object such as { "intent": "bump to next patch release" }, { "intent": "bump to next alpha version" }, or { "intent": "bump to 0.2.1" }.')
}

const pollIntervalMs = 60000
const maxPolls = 120
const releaseStatusPollIntervalMs = 60000
const releaseStatusMaxPolls = 60

phase('prepare')
log('Resolving Rust crate release intent')
log('Release request:', releaseRequest)

const resolvedIntent = await agent(
  `Resolve the user's natural-language Rust crate release request into a concrete next release version.

User request:
${releaseRequest}

Workflow args JSON; only the intent field is supported:
${JSON.stringify({ intent: releaseRequest }, null, 2)}

Repository context to inspect before deciding:
- root Cargo.toml [workspace.package].version
- rust/cli/Cargo.toml smol-workflow-engine dependency version
- existing git tags matching v*
- crates.io versions for smol-workflow-engine and smol-workflow-cli if useful

Interpretation rules:
- "bump to 0.2.1", "release 0.2.1", or "v0.2.1" means mode=explicit-version, version=0.2.1, bump=none.
- "next patch release" means mode=semver-bump, bump=patch, and version is current workspace version with the patch component incremented.
- "next minor release" means mode=semver-bump, bump=minor, and version is current workspace version with the minor component incremented and patch reset to 0.
- "next major release" means mode=semver-bump, bump=major, and version is current workspace version with the major component incremented and minor/patch reset to 0.
- "next alpha version", "alpha release", or "prerelease" means mode=alpha-bump, bump=alpha. Choose the next semver alpha prerelease using the current workspace version, existing tags, and crates.io versions. Prefer the next alpha for the next unreleased base version unless the user request or repository state clearly indicates another base.
- Strip any leading v from version. expectedTag must be v<version>.
- Do not choose a version that already has a git tag or is already published for either smol-workflow-engine or smol-workflow-cli unless the user explicitly asks for a retry of a partially-published release; if blocked, explain in assumptions/verification.
- Do not edit files, commit, tag, create PRs, or publish.

Set the structured intent field equal to the user request.

Return only the structured release intent.`,
  {
    phase: 'prepare',
    label: 'resolve-release-version',
    schema: RELEASE_INTENT_SCHEMA,
  },
)

log('Resolved release version:', resolvedIntent.version)
log('Expected tag:', resolvedIntent.expectedTag)

phase('release-prepare-code')
log('Updating release files and opening PR')

const codePrepReport = await agent(
  `Prepare the Rust crates.io release PR for the resolved version.

Original natural-language request:
${releaseRequest}

Resolved release intent JSON:
${JSON.stringify(resolvedIntent, null, 2)}

Requirements:
1. Inspect rust/AGENTS.md, root Cargo.toml, rust/cli/Cargo.toml, Cargo.lock, .github/workflows/cargo-publish.yml, .github/workflows/release.yml, current git status/diff, and existing git tags matching v*.
2. Verify the target version is ${resolvedIntent.version} and the expected tag is ${resolvedIntent.expectedTag}. If the target version is already tagged or already published for either smol-workflow-engine or smol-workflow-cli, stop and report the blocker.
3. Update root Cargo.toml [workspace.package].version to ${resolvedIntent.version}.
4. Update rust/cli/Cargo.toml so the smol-workflow-engine dependency version is ${resolvedIntent.version} while preserving the path dependency.
5. Run cargo check --workspace to update/verify Cargo.lock if needed.
6. Run release validation:
   - cargo fmt --all -- --check
   - cargo clippy --workspace --all-targets -- -D warnings
   - cargo test --workspace --all-targets
   - cargo publish -p smol-workflow-engine --dry-run --locked
   - If smol-workflow-engine ${resolvedIntent.version} is already available on crates.io, also run cargo publish -p smol-workflow-cli --dry-run --locked. Otherwise, run cargo package -p smol-workflow-cli --locked --list and explain that the CLI publish dry-run cannot fully resolve until the engine version exists on crates.io.
7. Preserve unrelated working-tree changes. Stage and commit only the Rust release files you intentionally changed.
8. Commit with message: release(rust): prepare ${resolvedIntent.expectedTag}
9. Push a branch named release/rust-${resolvedIntent.expectedTag} or a similarly clear unique branch name.
10. Create a non-draft PR against main.
11. The PR title must indicate the Rust crate release version.
12. The PR body must clearly state:
    - target version ${resolvedIntent.version};
    - expected tag ${resolvedIntent.expectedTag};
    - this workflow will create the GitHub Release/tag after the PR is merged;
    - creating that tag triggers both the binary release workflow and the crates.io publish workflow;
    - smol-workflow-engine is published before smol-workflow-cli because the CLI depends on the engine crate.
13. Do not create or push the release tag yourself.
14. If committing, pushing, or creating the PR is blocked by credentials/remotes/tools, do as much validation as possible and report the blocker in notes.

Return a structured report with release, expectedTag, branch, commit, prUrl, prNumber, filesChanged, validation, and notes. Use empty string / 0 for PR fields only if blocked.`,
  {
    phase: 'release-prepare-code',
    label: 'update-code-and-open-pr',
    schema: CODE_PREP_REPORT_SCHEMA,
  },
)

if (!codePrepReport.prUrl || !codePrepReport.prNumber) {
  throw new Error(`Release PR was not created: ${JSON.stringify(codePrepReport.notes)}`)
}

phase('release-prepare-pr')
log(`Waiting for release PR to merge: ${codePrepReport.prUrl}`)
log(`Polling up to ${maxPolls} times every ${pollIntervalMs}ms`)

let mergedPr = null
for (let attempt = 1; attempt <= maxPolls; attempt += 1) {
  log(`Checking release PR merge status, attempt ${attempt}/${maxPolls}`)
  const status = await agent(
    `Check whether the Rust crate release PR has merged.

PR URL: ${codePrepReport.prUrl}
PR number: ${codePrepReport.prNumber}
Expected release tag after merge: ${resolvedIntent.expectedTag}

Requirements:
1. Use the GitHub CLI or API if available to inspect the PR state.
2. Report merged=true only if the PR is merged into the base branch.
3. If merged, report the merge commit SHA and base branch.
4. If closed without merge, report state=closed, merged=false, and explain that finalization must not proceed.
5. Do not merge the PR yourself, do not create tags, and do not publish crates.

Return only the structured PR status.`,
    {
      phase: 'release-prepare-pr',
      label: `poll-release-pr-${attempt}`,
      schema: PR_STATUS_SCHEMA,
    },
  )

  if (status.merged) {
    mergedPr = status
    log(`Release PR merged at ${status.mergeCommit}`)
    break
  }

  if (status.state && status.state.toLowerCase() === 'closed') {
    throw new Error(`Release PR closed without merge: ${status.summary}`)
  }

  if (attempt < maxPolls) {
    log(`Release PR is not merged yet; sleeping for ${pollIntervalMs}ms`)
    await sleep(pollIntervalMs)
  }
}

if (!mergedPr) {
  throw new Error(`Timed out waiting for release PR to merge after ${maxPolls} polls: ${codePrepReport.prUrl}`)
}

phase('release-finalize')
log('Creating GitHub release on merged commit')

const finalReport = await agent(
  `Finalize the Rust crates.io release by creating the GitHub Release/tag on the merged commit.

Original natural-language request:
${releaseRequest}

Resolved release intent JSON:
${JSON.stringify(resolvedIntent, null, 2)}

Release PR preparation report JSON:
${JSON.stringify(codePrepReport, null, 2)}

Merged PR status JSON:
${JSON.stringify(mergedPr, null, 2)}

Requirements:
1. Verify the PR merged into the intended base branch and that mergeCommit is available.
2. Verify the root Cargo.toml at the merged commit has workspace package version ${resolvedIntent.version}, and rust/cli/Cargo.toml depends on smol-workflow-engine version ${resolvedIntent.version}.
3. Create the GitHub Release/tag ${resolvedIntent.expectedTag} targeting the merged commit SHA ${mergedPr.mergeCommit}. Use the GitHub CLI if available, for example gh release create ${resolvedIntent.expectedTag} --target ${mergedPr.mergeCommit} --generate-notes, unless a repository-specific release process requires equivalent commands.
4. Do not run cargo publish manually. The tag/GitHub Release should trigger .github/workflows/release.yml and .github/workflows/cargo-publish.yml.
5. After creating the release, do an initial inspection of the relevant workflow runs if readily available:
   - binary release workflow: .github/workflows/release.yml
   - crates.io publish workflow: .github/workflows/cargo-publish.yml
   A separate polling step will keep checking remote release status after this agent returns.
6. If the release/tag already exists, verify it points at the intended merged commit or report the mismatch as a blocker.
7. If credentials/remotes/tools prevent creating the GitHub Release, report the exact blocker and the command a human should run.

Return a structured report with release, expectedTag, releaseUrl, targetCommit, cratesPublishWorkflow, binaryReleaseWorkflow, validation, and notes.`,
  {
    phase: 'release-finalize',
    label: 'create-github-release',
    schema: FINAL_REPORT_SCHEMA,
  },
)

log(`Checking remote release status for ${resolvedIntent.expectedTag}`)
log(`Polling up to ${releaseStatusMaxPolls} times every ${releaseStatusPollIntervalMs}ms`)

let remoteReleaseStatus = null
for (let attempt = 1; attempt <= releaseStatusMaxPolls; attempt += 1) {
  log(`Inspecting remote release status, attempt ${attempt}/${releaseStatusMaxPolls}`)
  const status = await agent(
    `Inspect the remote status for the Rust crate release.

Expected tag: ${resolvedIntent.expectedTag}
Expected target commit: ${mergedPr.mergeCommit}
Release URL from creation step: ${finalReport.releaseUrl}

Requirements:
1. Use the GitHub CLI or API if available to verify that release/tag ${resolvedIntent.expectedTag} exists remotely.
2. Verify the release/tag points at target commit ${mergedPr.mergeCommit}, or report a mismatch in notes.
3. Inspect the workflow runs triggered by the tag for:
   - binary release workflow: .github/workflows/release.yml
   - crates.io publish workflow: .github/workflows/cargo-publish.yml
4. Set allRunsStarted=true only after both expected workflow runs are visible.
5. Set allRunsCompleted=true only after both expected workflow runs are terminal/completed.
6. Set allRunsSuccessful=true only after both expected workflow runs completed successfully.
7. If a workflow run failed, was cancelled, or the release/tag points at the wrong commit, report the failure in summary/notes.
8. Do not create releases, tags, or publish crates in this inspection step.

Return only the structured remote release status.`,
    {
      phase: 'release-finalize',
      label: `poll-remote-release-${attempt}`,
      schema: REMOTE_RELEASE_STATUS_SCHEMA,
    },
  )

  remoteReleaseStatus = status

  if (status.allRunsCompleted) {
    if (!status.allRunsSuccessful) {
      throw new Error(`Remote release workflows completed unsuccessfully: ${status.summary}`)
    }
    log('Remote release workflows completed successfully')
    break
  }

  if (attempt < releaseStatusMaxPolls) {
    log(`Remote release workflows are not complete yet; sleeping for ${releaseStatusPollIntervalMs}ms`)
    await sleep(releaseStatusPollIntervalMs)
  }
}

if (!remoteReleaseStatus || !remoteReleaseStatus.allRunsCompleted) {
  throw new Error(`Timed out waiting for remote release workflows to complete after ${releaseStatusMaxPolls} polls`)
}

phase('release-install-verify')
log(`Running install verification sub-workflow for ${resolvedIntent.expectedTag}`)

const installVerifyReport = await workflow(
  { scriptPath: './rust-release-install-verify.js' },
  {
    expectedTag: resolvedIntent.expectedTag,
    version: resolvedIntent.version,
    remoteReleaseStatus,
  },
)

if (!installVerifyReport.sandboxVmVerified || !installVerifyReport.versionMatchesLatestRelease) {
  throw new Error(`Latest release install verification failed: ${installVerifyReport.summary}`)
}

export default {
  releaseRequest,
  resolvedIntent,
  codePrepReport,
  mergedPr,
  finalReport,
  remoteReleaseStatus,
  installVerifyReport,
}
