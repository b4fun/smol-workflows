/** @type {import('@smol-workflows/sdk').WorkflowMetadata} */
export const meta = {
  name: 'exe-dev-sandbox-impl',
  description: 'Implement the Rust exe.dev sandbox provider from docs/experiments/exe-dev-sandbox.md, with validation phases between implementation milestones',
  phases: [
    { title: 'Orient', detail: 'Read the proposal and existing sandbox-provider implementations' },
    { title: 'Scaffold', detail: 'Create the Rust crate, binary, config/state skeleton, and JSONL serve loop' },
    { title: 'TestScaffold', detail: 'Run focused build/protocol tests and fix scaffold issues' },
    { title: 'Lifecycle', detail: 'Implement exe.dev SSH control-plane lifecycle and cleanup behavior' },
    { title: 'TestLifecycle', detail: 'Add fake-ssh lifecycle tests and run focused validation' },
    { title: 'WorkspaceFiles', detail: 'Implement workspace sync and file APIs' },
    { title: 'TestWorkspaceFiles', detail: 'Add file/workspace sync tests and run focused validation' },
    { title: 'ExecSpawn', detail: 'Implement SSH-backed exec/spawn and event streaming' },
    { title: 'TestExecSpawn', detail: 'Add exec/spawn tests and run focused validation' },
    { title: 'RealExeDevSmoke', detail: 'Validate against a real exe.dev VM' },
    { title: 'FixRealExeDevSmoke', detail: 'Fix and rerun failed real exe.dev smoke validation' },
    { title: 'DocsFinal', detail: 'Update docs and run final fake + real-validation checks' },
  ],
}

const PROPOSAL_PATH = 'docs/experiments/exe-dev-sandbox.md'
const WORKFLOW_PATH = '.agents/workflows/exe-dev-sandbox-impl.mjs'

const REPORT_SCHEMA = {
  type: 'object',
  properties: {
    summary: { type: 'string' },
    filesChanged: { type: 'array', items: { type: 'string' } },
    testsRun: { type: 'array', items: { type: 'string' } },
    testsPassed: { type: 'boolean' },
    followUps: { type: 'array', items: { type: 'string' } },
  },
  required: ['summary', 'filesChanged', 'testsRun', 'testsPassed', 'followUps'],
}

const TEST_REPORT_SCHEMA = {
  type: 'object',
  properties: {
    summary: { type: 'string' },
    testsRun: { type: 'array', items: { type: 'string' } },
    passed: { type: 'boolean' },
    fixesApplied: { type: 'array', items: { type: 'string' } },
    remainingFailures: { type: 'array', items: { type: 'string' } },
    filesChanged: { type: 'array', items: { type: 'string' } },
  },
  required: ['summary', 'testsRun', 'passed', 'fixesApplied', 'remainingFailures', 'filesChanged'],
}

const FINAL_SCHEMA = {
  type: 'object',
  properties: {
    summary: { type: 'string' },
    implemented: { type: 'array', items: { type: 'string' } },
    filesChanged: { type: 'array', items: { type: 'string' } },
    validation: { type: 'array', items: { type: 'string' } },
    knownLimitations: { type: 'array', items: { type: 'string' } },
    nextSteps: { type: 'array', items: { type: 'string' } },
  },
  required: ['summary', 'implemented', 'filesChanged', 'validation', 'knownLimitations', 'nextSteps'],
}

const DEFAULT_FINAL_CHECKS = [
  'cargo fmt --check',
  'cargo test -p smol-workflow-sandbox',
  'cargo test -p smol-sandbox-exe-dev',
  'cargo check --workspace',
]

function asList(value, fallback) {
  if (Array.isArray(value)) return value.map(String).filter(Boolean)
  if (typeof value === 'string' && value.trim()) return value.split(',').map(s => s.trim()).filter(Boolean)
  return fallback
}

const finalChecks = asList(args?.finalChecks, DEFAULT_FINAL_CHECKS)
const skipRealExeDevSmoke = args?.skipRealExeDevSmoke === true
  || String(args?.skipRealExeDevSmoke ?? '').toLowerCase() === 'true'
  || String(args?.skipRealExeDevSmoke ?? '') === '1'
const realExeDevSmokeRequired = !skipRealExeDevSmoke

const scope = typeof args?.scope === 'string' && args.scope.trim()
  ? args.scope.trim()
  : 'Implement the MVP described in the proposal using Rust. Use fake ssh tests for deterministic automated coverage, and run a real exe.dev VM smoke validation by default. Do not claim the provider is validated against exe.dev unless a real VM was actually created, reached over SSH, and deleted or intentionally preserved.'

function commonInstructions(extra = '') {
  return `You are implementing code in this repository.

Read ${PROPOSAL_PATH}, ${WORKFLOW_PATH}, rust/sandbox/README.md, rust/sandbox/src/v1.rs, rust/sandbox/src/jsonl.rs, sandbox-providers/azure-sandbox, and sandbox-providers/local-worktree as needed.

Follow repository style. Prefer precise, minimal changes. Use Rust for the exe.dev provider. Keep the existing sandbox JSONL protocol unchanged. Do not require real exe.dev credentials for normal deterministic tests. Use fake ssh/test fixtures for automated coverage, but do not let fake fixtures be the only source of truth for exe.dev CLI syntax, VM naming limits, or remote image filesystem assumptions. Do not put secrets in logs.

Real exe.dev validation policy:
- Run a real VM smoke validation by default. The only workflow-level opt-out is args.skipRealExeDevSmoke=true.
- A real smoke validation should create exactly one provider VM, print/record its VM name and ssh_dest, verify it appears in "ssh exe.dev ls --json", verify direct SSH works, verify the configured cwd can be created/used, then close/delete it by default.
- Preserve the VM only when an explicit keep/debug env such as SMOL_EXE_DEV_KEEP=1 is set, and clearly report the preserved VM name.
- If real validation is skipped by explicit opt-out or cannot run due to an external blocker, say so explicitly and do not claim exe.dev runtime compatibility was verified.

Scope: ${scope}

${extra}`
}

phase('Orient')
log('Reading proposal and current sandbox provider model')
const orientation = await agent(commonInstructions(`Inspect the current codebase and produce an implementation map only; do not edit files in this phase.

Return:
- where the new crate should live;
- which workspace files must change;
- which protocol types to reuse;
- the test strategy, especially fake ssh;
- any adjustments needed to the proposal before implementation.`), {
  label: 'orient-exe-dev-sandbox-provider',
  phase: 'Orient',
})

phase('Scaffold')
log('Creating Rust crate and JSONL provider skeleton')
const scaffold = await agent(commonInstructions(`Implement milestone 1.

Use this orientation context:
${orientation}

Required changes:
- Add rust/sandbox-providers/exe-dev as a workspace member.
- Add binary smol-sandbox-exe-dev.
- Implement a long-lived JSONL serve loop for at least capabilities and shutdown.
- Add config parsing structs and default config path resolution.
- Add provider error helpers and state skeleton.
- Add initial tests that do not contact exe.dev.

After editing, run focused validation such as cargo fmt and cargo test for the new crate if possible.

Return a structured report.`), {
  label: 'scaffold-exe-dev-provider',
  phase: 'Scaffold',
  schema: REPORT_SCHEMA,
})

phase('TestScaffold')
log('Validating scaffold before lifecycle work')
const scaffoldTest = await agent(commonInstructions(`Validate and fix the scaffold from the prior phase.

Prior report:
${JSON.stringify(scaffold, null, 2)}

Run focused checks:
- cargo fmt --check, or cargo fmt followed by cargo fmt --check if formatting is needed;
- cargo test -p smol-sandbox-exe-dev, if the package exists;
- cargo check -p smol-sandbox-exe-dev, if tests are not yet available.

Fix failures that are within the scaffold scope. Do not implement lifecycle yet except where needed for compilation.

Return a structured test report.`), {
  label: 'test-scaffold-exe-dev-provider',
  phase: 'TestScaffold',
  schema: TEST_REPORT_SCHEMA,
})

phase('Lifecycle')
log('Implementing exe.dev lifecycle over SSH control plane')
const lifecycle = await agent(commonInstructions(`Implement milestone 2.

Prior test report:
${JSON.stringify(scaffoldTest, null, 2)}

Required behavior:
- Implement SSH control plane for exe.dev lifecycle: new, ls lookup if needed, rm.
- Generate safe VM names with a smol-workflows/provider prefix.
- Wait for direct VM SSH readiness with bounded retry/backoff.
- Implement open, close, cleanup_group, and local provider state persistence where practical.
- Keep-on-close/debug behavior should be profile/env controlled.
- Do not implement file APIs or exec beyond stubs needed for clean errors.
- Add fake ssh support in tests for deterministic coverage.
- Do not hard-code proposal-era exe.dev assumptions into fake fixtures without checking current CLI/help output; real behavior is validated in the RealExeDevSmoke phase.

Run focused tests and return a structured report.`), {
  label: 'lifecycle-exe-dev-provider',
  phase: 'Lifecycle',
  schema: REPORT_SCHEMA,
})

phase('TestLifecycle')
log('Testing lifecycle with fake ssh')
const lifecycleTest = await agent(commonInstructions(`Validate and fix lifecycle behavior.

Prior report:
${JSON.stringify(lifecycle, null, 2)}

Add or improve tests that use a fake ssh executable earlier in PATH and simulate the current exe.dev command shapes, including:
- ssh exe.dev new --name <name> --image ... --json;
- ssh exe.dev ls --json;
- ssh exe.dev rm <name> --json;
- ssh <vm>.exe.xyz true readiness.

The fake fixture should mirror known current exe.dev constraints where practical, including VM-name length/format validation and stdout JSON error bodies on failed control-plane commands.

Run focused lifecycle tests. Fix failures. Keep this phase deterministic; real exe.dev validation is handled in the RealExeDevSmoke phase.

Return a structured test report.`), {
  label: 'test-lifecycle-exe-dev-provider',
  phase: 'TestLifecycle',
  schema: TEST_REPORT_SCHEMA,
})

phase('WorkspaceFiles')
log('Implementing workspace sync and file methods')
const workspaceFiles = await agent(commonInstructions(`Implement milestone 3.

Prior test report:
${JSON.stringify(lifecycleTest, null, 2)}

Required behavior:
- Implement tar-over-ssh workspace upload for the default sync mode.
- Create the sandbox cwd during open before sync.
- Implement create_temp_dir, create_dir_all, write_file, read_file, and remove.
- Decode/encode JSONL base64 file content correctly.
- Resolve relative paths against session cwd.
- Keep operations binary-safe where possible.
- Keep git-sync as optional future work unless easy; do not regress the tar MVP.

Add fake ssh tests for file APIs and workspace sync. Run focused tests and return a structured report.`), {
  label: 'workspace-files-exe-dev-provider',
  phase: 'WorkspaceFiles',
  schema: REPORT_SCHEMA,
})

phase('TestWorkspaceFiles')
log('Testing workspace sync and file APIs')
const workspaceFilesTest = await agent(commonInstructions(`Validate and fix workspace sync/file behavior.

Prior report:
${JSON.stringify(workspaceFiles, null, 2)}

Run tests that prove:
- workspace sync invokes tar/ssh as expected or is testably abstracted;
- write_file accepts base64 and sends bytes to the remote path;
- read_file returns content_base64;
- remove treats missing paths as success where possible;
- relative paths are resolved using session cwd.

Fix failures. Do not implement exec/spawn except where needed for compilation.

Return a structured test report.`), {
  label: 'test-workspace-files-exe-dev-provider',
  phase: 'TestWorkspaceFiles',
  schema: TEST_REPORT_SCHEMA,
})

phase('ExecSpawn')
log('Implementing SSH-backed exec/spawn')
const execSpawn = await agent(commonInstructions(`Implement milestones 4 and 5.

Prior test report:
${JSON.stringify(workspaceFilesTest, null, 2)}

Required behavior:
- Implement foreground exec over direct VM SSH.
- Validate argv is non-empty.
- Apply cwd and env using careful shell quoting.
- Support stdin by piping to ssh or by writing a temp file remotely.
- Stream started/stdout/stderr/exited JSONL events and return accumulated stdout/stderr base64.
- Implement MVP spawn with nohup/PID and track PIDs for close cleanup where practical.
- Add tests using fake ssh/process fixtures.

Do not build the remote helper in this milestone unless the MVP is already stable.

Run focused tests and return a structured report.`), {
  label: 'exec-spawn-exe-dev-provider',
  phase: 'ExecSpawn',
  schema: REPORT_SCHEMA,
})

phase('TestExecSpawn')
log('Testing exec/spawn and event streaming')
const execSpawnTest = await agent(commonInstructions(`Validate and fix exec/spawn behavior.

Prior report:
${JSON.stringify(execSpawn, null, 2)}

Run tests that prove:
- exec streams stdout/stderr events before final result;
- exit_code/stdout_base64/stderr_base64 are correct;
- stdin is handled;
- env/cwd are represented in the remote command safely;
- spawn returns a process_id when fake ssh outputs a PID;
- close attempts cleanup for tracked spawned PIDs where implemented.

Fix failures. Keep this phase focused on deterministic fake-ssh/process fixtures; real exe.dev validation is handled in the next phase.

Return a structured test report.`), {
  label: 'test-exec-spawn-exe-dev-provider',
  phase: 'TestExecSpawn',
  schema: TEST_REPORT_SCHEMA,
})

phase('RealExeDevSmoke')
log('Validating against a real exe.dev VM')
const realExeDevSmoke = await agent(commonInstructions(`Validate the provider against current real exe.dev behavior, not just fake ssh.

Prior fake-ssh test report:
${JSON.stringify(execSpawnTest, null, 2)}

Real validation is required for this workflow run: ${realExeDevSmokeRequired}.
Explicit skip arg args.skipRealExeDevSmoke: ${skipRealExeDevSmoke}.

Run a real exe.dev smoke validation unless args.skipRealExeDevSmoke is true. Use a direct provider JSONL smoke script/command for this workflow-level validation rather than relying on any separately gated test.

Required real smoke behavior when enabled:
- Confirm "ssh exe.dev ls --json" works.
- Inspect current exe.dev CLI syntax with "ssh exe.dev new --help" before relying on proposal-era command shapes.
- Build the provider binary if needed.
- Create exactly one real exe.dev VM through "smol-sandbox-exe-dev serve" using the same JSONL "open" path the runtime uses.
- Print and record the provider VM name, ssh_dest, session id, and cwd.
- Assert the VM appears in "ssh exe.dev ls --json" while open or intentionally kept.
- Verify direct SSH to the VM works using a safe command form such as "ssh <ssh_dest> -- pwd" and "ssh <ssh_dest> -- hostname".
- Verify the configured cwd is user-writable/usable, because fake tests can miss image permission issues.
- Close/delete the VM by default and verify it no longer appears in "ssh exe.dev ls --json".
- If SMOL_EXE_DEV_KEEP=1 or a keep/debug profile is intentionally used, preserve the VM and report the exact VM name and deletion command.

If real validation was explicitly skipped with args.skipRealExeDevSmoke=true or cannot run due to an external blocker, do not create VMs. Return passed=false and include a remaining failure stating why real exe.dev validation did not run, so the final phase cannot accidentally claim runtime compatibility.

Return a structured test report. Include VM names and cleanup outcome in the summary/fixes fields without logging secrets.`), {
  label: 'real-exe-dev-smoke-provider',
  phase: 'RealExeDevSmoke',
  schema: TEST_REPORT_SCHEMA,
})

let fixRealExeDevSmoke = {
  summary: 'Skipped fix/reverify phase because the initial real exe.dev smoke passed or real validation was explicitly skipped.',
  testsRun: [],
  passed: true,
  fixesApplied: [],
  remainingFailures: [],
  filesChanged: [],
}

if (realExeDevSmokeRequired && realExeDevSmoke?.passed === false) {
  phase('FixRealExeDevSmoke')
  log('Fixing failed real exe.dev smoke validation and rerunning it')
  fixRealExeDevSmoke = await agent(commonInstructions(`The real exe.dev smoke validation was explicitly requested and failed. Diagnose, fix, and rerun it.

Initial real smoke report:
${JSON.stringify(realExeDevSmoke, null, 2)}

Required behavior:
- First ensure no provider-created orphan VM from the failed smoke is left behind. Only clean up VMs that are clearly owned by this provider/run, such as names with the smol-workflows-exe-dev prefix reported by the smoke phase.
- Determine whether the failure is a fixable implementation/config/test issue or an external blocker.
- Fix implementation issues such as stale exe.dev CLI syntax, VM-name constraints, cwd/permission assumptions, command quoting, missing "--" for direct SSH command probes, workspace sync assumptions, or incomplete error diagnostics.
- Do not attempt to fix external blockers such as missing exe.dev account access, SSH authentication failures, quota/billing errors, or network outages; report them as remaining failures.
- After each fix, rerun the real smoke validation path directly.
- Stop when the real smoke passes, or when the remaining failure is clearly external/non-actionable in this repository.
- By default, delete the real VM after the passing rerun and verify it no longer appears in "ssh exe.dev ls --json". Preserve it only if SMOL_EXE_DEV_KEEP=1 is explicitly set, and report the exact VM name and deletion command.

Return a structured test report. The report must say whether real exe.dev validation finally passed, list fixes applied, tests/commands run, any remaining failures, and the cleanup outcome.`), {
    label: 'fix-real-exe-dev-smoke-provider',
    phase: 'FixRealExeDevSmoke',
    schema: TEST_REPORT_SCHEMA,
  })
}

phase('DocsFinal')
log('Updating docs and running final validation')
const final = await agent(commonInstructions(`Finalize the implementation.

Prior fake-ssh test report:
${JSON.stringify(execSpawnTest, null, 2)}

Initial real exe.dev smoke report:
${JSON.stringify(realExeDevSmoke, null, 2)}

Fix/reverify real exe.dev smoke report:
${JSON.stringify(fixRealExeDevSmoke, null, 2)}

Required finalization:
- Update ${PROPOSAL_PATH} if implementation details differ from the proposal.
- Add a README or usage docs for the new provider if appropriate.
- Ensure Cargo workspace metadata is coherent.
- Run final checks, or the closest practical subset, and fix issues:
${finalChecks.map(cmd => `  - ${cmd}`).join('\n')}

If a check cannot run in this environment, record why. Treat real exe.dev validation as passed only if either realExeDevSmoke.passed or fixRealExeDevSmoke.passed is true after an actually enabled real-VM run. If both real reports are failed/skipped, list real exe.dev validation as a known limitation/next step and do not state that runtime compatibility with current exe.dev was verified. If real validation passed, include the VM name/ssh_dest evidence and cleanup outcome in validation.

Return the final structured report.`), {
  label: 'finalize-exe-dev-provider',
  phase: 'DocsFinal',
  schema: FINAL_SCHEMA,
})

export default {
  orientation,
  scaffold,
  scaffoldTest,
  lifecycle,
  lifecycleTest,
  workspaceFiles,
  workspaceFilesTest,
  execSpawn,
  execSpawnTest,
  realExeDevSmoke,
  fixRealExeDevSmoke,
  final,
}
