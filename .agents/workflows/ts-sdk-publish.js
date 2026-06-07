export const meta = {
  name: 'ts-sdk-publish',
  description: 'Prepare a TypeScript SDK release PR: changelog, version/tag selection, validation, commit, and draft PR',
  phases: [
    { title: 'Prepare', detail: 'Read release intent and set release scope', model: 'gpt-5.4-mini' },
    { title: 'ReleasePrep', detail: 'Update changelog and package version in parallel', model: 'gpt-5.4-mini' },
    { title: 'Finalize', detail: 'Validate, commit, and create a draft release PR', model: 'gpt-5.5' },
  ],
}

const TASK_REPORT_SCHEMA = {
  type: 'object',
  properties: {
    summary: { type: 'string' },
    filesChanged: { type: 'array', items: { type: 'string' } },
    verification: { type: 'array', items: { type: 'string' } },
    skipped: {
      type: 'array',
      items: {
        type: 'object',
        properties: {
          item: { type: 'string' },
          reason: { type: 'string' },
        },
        required: ['item', 'reason'],
      },
    },
  },
  required: ['summary', 'filesChanged', 'verification', 'skipped'],
}

const VERSION_REPORT_SCHEMA = {
  type: 'object',
  properties: {
    releaseKind: {
      type: 'string',
      enum: ['alpha', 'normal'],
      description: 'The explicit release type requested by workflow input.',
    },
    packageVersion: { type: 'string' },
    expectedTag: {
      type: 'string',
      description: 'Expected git tag to create after the PR merges, for example ts/sdk/v0.1.0-alpha.3.',
    },
    npmDistTag: { type: 'string' },
    summary: { type: 'string' },
    filesChanged: { type: 'array', items: { type: 'string' } },
    verification: { type: 'array', items: { type: 'string' } },
    skipped: {
      type: 'array',
      items: {
        type: 'object',
        properties: {
          item: { type: 'string' },
          reason: { type: 'string' },
        },
        required: ['item', 'reason'],
      },
    },
  },
  required: ['releaseKind', 'packageVersion', 'expectedTag', 'npmDistTag', 'summary', 'filesChanged', 'verification', 'skipped'],
}

const FINAL_REPORT_SCHEMA = {
  type: 'object',
  properties: {
    release: { type: 'string' },
    expectedTag: { type: 'string' },
    branch: { type: 'string' },
    commit: { type: 'string' },
    prUrl: { type: 'string' },
    validation: { type: 'array', items: { type: 'string' } },
    notes: { type: 'array', items: { type: 'string' } },
  },
  required: ['release', 'expectedTag', 'branch', 'commit', 'prUrl', 'validation', 'notes'],
}

const rawReleaseInput = args && typeof args === 'object' ? args : {}
const rawReleaseType = typeof rawReleaseInput.releaseType === 'string'
  ? rawReleaseInput.releaseType.trim().toLowerCase()
  : ''

const releaseTypeAliases = {
  alpha: 'alpha',
  prerelease: 'alpha',
  pre: 'alpha',
  dev: 'alpha',
  normal: 'normal',
  official: 'normal',
  stable: 'normal',
  latest: 'normal',
  production: 'normal',
}

const releaseType = releaseTypeAliases[rawReleaseType]

if (!releaseType) {
  throw new Error('Missing or invalid required arg releaseType. Pass --args-releaseType alpha or --args-releaseType normal.')
}

const releaseInput = {
  ...rawReleaseInput,
  releaseType,
}

const releaseIntent = [
  `releaseType=${releaseType}`,
  releaseInput.version,
  releaseInput.notes,
]
  .filter(value => value !== undefined && value !== null)
  .map(value => typeof value === 'string' ? value : JSON.stringify(value))
  .join(' ')
  .trim() || JSON.stringify(releaseInput)

phase('Prepare')
log('Preparing TypeScript SDK release workflow')
log('Release type:', releaseType)
log('Release input:', releaseIntent)

phase('ReleasePrep')
log('Updating changelog and package version in parallel')

const [changelogReport, versionReport] = await parallel([
  () => agent(
    `You are preparing a release PR for the TypeScript SDK package in this repository.

Task: check and update only ts/sdk/changelogs.md.

Requirements:
- Inspect ts/sdk/AGENTS.md, ts/sdk/changelogs.md, current git diff/status for ts/sdk, and relevant recent commits/tags if needed.
- Ensure ts/sdk/changelogs.md accurately captures all user-visible TypeScript SDK changes that should be released.
- Keep the existing changelog style.
- Because package version selection runs in parallel, do not edit package.json and do not guess a final version unless the workflow input explicitly provides one.
- If a final version is not yet known, keep release notes under the Unpublished section; the final validation step may move them under the selected version.
- Do not edit files outside ts/sdk/changelogs.md.
- Do not remove / override published versions from ts/sdk/changelogs.md.
- Preserve unrelated working-tree changes.

Workflow input JSON:
${JSON.stringify(releaseInput, null, 2)}

Return a structured report with summary, filesChanged, verification, and skipped.`,
    {
      phase: 'ReleasePrep',
      label: 'update-ts-sdk-changelog',
      schema: TASK_REPORT_SCHEMA,
    },
  ),
  () => agent(
    `You are preparing a release PR for the TypeScript SDK package in this repository.

Task: infer the next package version/tag from the explicit releaseType arg, then update ts/sdk/package.json and ts/sdk/package-lock.json consistently.

Requirements:
- Inspect ts/sdk/AGENTS.md, ts/sdk/package.json, ts/sdk/package-lock.json, .github/workflows/npm-sdk-publish.yml, and git tags matching ts/sdk/v*.
- Use releaseInput.releaseType exactly; it is already validated and is either alpha or normal.
- For alpha releases, choose the next semver alpha version after existing package versions and tags, preserving the package's semver line unless the input explicitly requests a base version.
- For normal releases, choose the appropriate stable semver version. If the current version is an alpha for the same base, release that base without the prerelease suffix; otherwise use semver rules and the input.
- Update both package.json and package-lock.json. Prefer npm version <version> --no-git-tag-version from ts/sdk when practical.
- Report the expected post-merge git tag as ts/sdk/v<package-version>.
- Report npmDistTag as alpha for alpha prereleases and latest for normal releases.
- Do not create a git tag. Do not edit changelogs.md. Do not edit files outside ts/sdk/package.json and ts/sdk/package-lock.json.
- Preserve unrelated working-tree changes.

Workflow input JSON:
${JSON.stringify(releaseInput, null, 2)}

Return a structured report with releaseKind set to releaseInput.releaseType, plus packageVersion, expectedTag, npmDistTag, summary, filesChanged, verification, and skipped.`,
    {
      phase: 'ReleasePrep',
      label: 'update-ts-sdk-version',
      schema: VERSION_REPORT_SCHEMA,
    },
  ),
])

phase('Finalize')
log('Validating release preparation, committing, and creating draft PR')

const finalReport = await agent(
  `You are finalizing a TypeScript SDK release PR.

Context:
- The changelog update task and version/tag task have completed.
- Validate the resulting changes, commit them, and create a draft pull request for the release.

Workflow input JSON:
${JSON.stringify(releaseInput, null, 2)}

Changelog task report JSON:
${JSON.stringify(changelogReport, null, 2)}

Version task report JSON:
${JSON.stringify(versionReport, null, 2)}

Requirements:
1. Inspect git status and the full diff for ts/sdk release files.
2. Determine the selected package version and expected tag from ts/sdk/package.json and the version task report. The expected tag must be ts/sdk/v<package-version>.
3. Ensure ts/sdk/changelogs.md has a release section for the selected package version. If release notes are still under Unpublished, move the release-ready notes into a ## <package-version> section while preserving the Unpublished heading for future changes.
4. Ensure ts/sdk/package.json and ts/sdk/package-lock.json versions match the selected package version.
5. Run release validation from ts/sdk: npm run typecheck and npm run build. If build changes dist files, include those dist changes when they are release-relevant.
6. Do not stage or commit unrelated files outside ts/sdk. Preserve unrelated working-tree changes. If there are pre-existing non-sdk changes, leave them unstaged.
7. Commit the SDK release changes with message: release(ts-sdk): prepare <package-version>
8. Create a draft PR for the release. Use a branch name like release/ts-sdk-v<package-version> if a new branch is needed.
9. The draft PR title must indicate the SDK release version.
10. The draft PR body must clearly state the release and must say that after merging the PR we need to create the expected tag: ts/sdk/v<package-version>. Include the expected tag in the body.
11. Do not create or push the release tag yourself.
12. If committing or creating the draft PR is not possible because credentials/remotes/tools are unavailable, do as much validation as possible and report the blocker in notes.

Return a structured report with release, expectedTag, branch, commit, prUrl, validation, and notes. Use an empty string for commit or prUrl only if blocked.`,
  {
    phase: 'Finalize',
    label: 'validate-commit-draft-pr',
    schema: FINAL_REPORT_SCHEMA,
  },
)

export default {
  releaseInput,
  releaseType,
  releaseIntent,
  changelogReport,
  versionReport,
  finalReport,
}
