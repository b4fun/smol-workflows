/** @type {import('@smol-workflows/sdk').WorkflowMetadata} */
export const meta = {
  name: 'exe-dev-provider-e2e',
  description: 'Validate the Rust exe.dev sandbox provider against real exe.dev VMs.',
  whenToUse: 'Use before merging or releasing provider changes that must be verified against current exe.dev behavior, not only fake SSH fixtures.',
  phases: [
    { title: 'Plan', detail: 'Confirm parameters and safety policy for real VM validation' },
    { title: 'Real exe.dev E2E', detail: 'Create at least five real exe.dev VMs through the provider JSONL path and validate operations' },
    { title: 'Report', detail: 'Summarize VM evidence, cleanup outcome, failures, and follow-ups' },
  ],
}

const profile = typeof args.profile === 'string' && args.profile.trim()
  ? args.profile.trim()
  : 'exe-dev/default'
const preserveVm = args.preserveVm === true
  || String(args.preserveVm ?? '').toLowerCase() === 'true'
  || String(args.preserveVm ?? '') === '1'
const buildProvider = args.buildProvider === false
  || String(args.buildProvider ?? '').toLowerCase() === 'false'
  || String(args.buildProvider ?? '') === '0'
  ? false
  : true
const providerBinary = typeof args.providerBinary === 'string' && args.providerBinary.trim()
  ? args.providerBinary.trim()
  : 'target/debug/smol-sandbox-exe-dev'
const sandboxGroupId = typeof args.sandboxGroupId === 'string' && args.sandboxGroupId.trim()
  ? args.sandboxGroupId.trim()
  : 'sbxgrp_exe_dev_e2e'
const workspaceFile = typeof args.workspaceFile === 'string' && args.workspaceFile.trim()
  ? args.workspaceFile.trim()
  : 'workspace-marker.txt'
const agentProvider = typeof args.agentProvider === 'string' && args.agentProvider.trim()
  ? args.agentProvider.trim()
  : 'pi'
const requestedVmCount = typeof args.vmCount === 'number'
  ? args.vmCount
  : typeof args.vmCount === 'string' && args.vmCount.trim()
    ? +args.vmCount
    : 5
const vmCount = requestedVmCount >= 5 ? requestedVmCount : 5

/** @satisfies {import('@smol-workflows/sdk').JSONSchema} */
const PLAN_SCHEMA = {
  type: 'object',
  properties: {
    profile: { type: 'string' },
    preserveVm: { type: 'boolean' },
    providerBinary: { type: 'string' },
    sandboxGroupId: { type: 'string' },
    expectedVmCount: { type: 'integer', minimum: 5 },
    safetyNotes: { type: 'array', items: { type: 'string' } },
  },
  required: ['profile', 'preserveVm', 'providerBinary', 'sandboxGroupId', 'expectedVmCount', 'safetyNotes'],
  additionalProperties: false,
}

/** @satisfies {import('@smol-workflows/sdk').JSONSchema} */
const E2E_SCHEMA = {
  type: 'object',
  properties: {
    passed: { type: 'boolean' },
    vmName: { type: ['string', 'null'] },
    sshDest: { type: ['string', 'null'] },
    sessionId: { type: ['string', 'null'] },
    cwd: { type: ['string', 'null'] },
    vmCount: { type: 'integer', minimum: 5 },
    vms: {
      type: 'array',
      minItems: 5,
      items: {
        type: 'object',
        properties: {
          vmName: { type: 'string' },
          sshDest: { type: 'string' },
          sessionId: { type: 'string' },
          cwd: { type: 'string' },
          deleted: { type: 'boolean' },
          preserved: { type: 'boolean' },
        },
        required: ['vmName', 'sshDest', 'sessionId', 'cwd', 'deleted', 'preserved'],
        additionalProperties: false,
      },
    },
    providerBinary: { type: 'string' },
    commandsRun: { type: 'array', items: { type: 'string' } },
    commandResults: {
      type: 'array',
      items: {
        type: 'object',
        properties: {
          command: { type: 'string' },
          exitCode: { type: ['integer', 'null'] },
          stdout: { type: 'string' },
          stderr: { type: 'string' },
          note: { type: 'string' },
        },
        required: ['command', 'exitCode', 'stdout', 'stderr', 'note'],
        additionalProperties: false,
      },
    },
    checks: {
      type: 'array',
      items: {
        type: 'object',
        properties: {
          name: { type: 'string' },
          passed: { type: 'boolean' },
          detail: { type: 'string' },
        },
        required: ['name', 'passed', 'detail'],
        additionalProperties: false,
      },
    },
    cleanup: {
      type: 'object',
      properties: {
        attempted: { type: 'boolean' },
        deleted: { type: 'boolean' },
        preserved: { type: 'boolean' },
        deletionCommand: { type: ['string', 'null'] },
        detail: { type: 'string' },
      },
      required: ['attempted', 'deleted', 'preserved', 'deletionCommand', 'detail'],
      additionalProperties: false,
    },
    failures: { type: 'array', items: { type: 'string' } },
    rawEvidence: { type: 'array', items: { type: 'string' } },
  },
  required: ['passed', 'vmName', 'sshDest', 'sessionId', 'cwd', 'vmCount', 'vms', 'providerBinary', 'commandsRun', 'commandResults', 'checks', 'cleanup', 'failures', 'rawEvidence'],
  additionalProperties: false,
}

/** @satisfies {import('@smol-workflows/sdk').JSONSchema} */
const FINAL_SCHEMA = {
  type: 'object',
  properties: {
    passed: { type: 'boolean' },
    summary: { type: 'string' },
    vmName: { type: ['string', 'null'] },
    sshDest: { type: ['string', 'null'] },
    vms: {
      type: 'array',
      minItems: 5,
      items: {
        type: 'object',
        properties: {
          vmName: { type: 'string' },
          sshDest: { type: 'string' },
          sessionId: { type: 'string' },
          cwd: { type: 'string' },
          deleted: { type: 'boolean' },
          preserved: { type: 'boolean' },
        },
        required: ['vmName', 'sshDest', 'sessionId', 'cwd', 'deleted', 'preserved'],
        additionalProperties: false,
      },
    },
    cleanupOutcome: { type: 'string' },
    commandsRun: { type: 'array', items: { type: 'string' } },
    commandResults: {
      type: 'array',
      items: {
        type: 'object',
        properties: {
          command: { type: 'string' },
          exitCode: { type: ['integer', 'null'] },
          stdout: { type: 'string' },
          stderr: { type: 'string' },
          note: { type: 'string' },
        },
        required: ['command', 'exitCode', 'stdout', 'stderr', 'note'],
        additionalProperties: false,
      },
    },
    failures: { type: 'array', items: { type: 'string' } },
    nextSteps: { type: 'array', items: { type: 'string' } },
  },
  required: ['passed', 'summary', 'vmName', 'sshDest', 'vms', 'cleanupOutcome', 'commandsRun', 'commandResults', 'failures', 'nextSteps'],
  additionalProperties: false,
}

phase('Plan')
log(`Preparing real exe.dev provider E2E for ${profile}; vmCount=${vmCount}; preserveVm=${preserveVm}`)

const plan = await agent(
  [
    'Prepare to validate the Rust exe.dev sandbox provider against real exe.dev infrastructure.',
    '',
    `This workflow intentionally creates ${vmCount} real remote exe.dev VMs. Do not replace this with fake SSH tests.`,
    `Use exactly ${vmCount} provider-created VMs for the smoke unless you must clean up a clearly identified provider-created orphan from this run.`,
    '',
    `Profile: ${profile}`,
    `Provider binary: ${providerBinary}`,
    `Sandbox group id: ${sandboxGroupId}`,
    `VM count: ${vmCount}`,
    `Build provider first: ${buildProvider}`,
    `Preserve VM for debugging: ${preserveVm}`,
    '',
    'Return the safety plan and do not run commands in this planning step.',
  ].join('\n'),
  { phase: 'Plan', provider: agentProvider, schema: PLAN_SCHEMA },
)

phase('Real exe.dev E2E')
log(`Creating ${vmCount} real VMs through smol-sandbox-exe-dev JSONL open`)

const e2e = await agent(
  [
    `Run the real exe.dev provider E2E now. You are authorized to create exactly ${vmCount} real exe.dev VMs for this validation.`,
    '',
    'Do not use fake SSH fixtures. Do not merely run cargo tests that skip real VM creation. The validation must exercise the provider JSONL serve/open path against exe.dev.',
    '',
    'Required high-level steps:',
    '1. Confirm exe.dev SSH access with `ssh exe.dev ls --json`.',
    '2. Inspect current create syntax with `ssh exe.dev new --help` and record the relevant syntax evidence.',
    buildProvider ? '3. Build the provider with `cargo build -p smol-sandbox-exe-dev`.' : '3. Do not build; use the provided provider binary as-is.',
    '4. Create a temporary local workspace containing a marker file.',
    '5. Start `smol-sandbox-exe-dev serve` and send JSONL requests directly to it.',
    `6. Open profile through JSONL \`open\` exactly ${vmCount} times. Use distinct sandbox group ids by appending an index to the supplied sandbox group id, such as <base>-1 through <base>-${vmCount}, so cleanup/state is easy to correlate.`,
    '7. For every opened VM, record `session.id`, `provider_session_id`, `cwd`, and provider state `ssh_dest`.',
    '8. Verify every VM appears in `ssh exe.dev ls --json` while it exists.',
    '9. Verify direct SSH works for every VM with safe command forms such as `ssh <ssh_dest> -- pwd` and `ssh <ssh_dest> -- hostname`.',
    '10. On every VM, verify provider file APIs over JSONL: create_dir_all, write_file, read_file, remove, and create_temp_dir.',
    '11. On every VM, verify provider exec over JSONL by running a simple command in the sandbox cwd and checking stdout/exit code.',
    '12. On at least one VM, verify spawn returns a PID with a short-lived or harmless command; if you skip spawn on any VM, explain why.',
    preserveVm
      ? `13. Preserve all ${vmCount} VMs for debugging by using SMOL_EXE_DEV_KEEP=1 or a keep cleanup policy; report every VM name and deletion command.`
      : `13. Close all ${vmCount} sessions through JSONL \`close\`, verify every VM is deleted, then send JSONL \`shutdown\`.`,
    '',
    'Implementation guidance:',
    '- Prefer a short Python or shell harness in a temp directory so the JSONL requests are reproducible.',
    '- Use the provider binary path below. If it is relative, resolve it from the repository root.',
    '- For preserve mode, set `SMOL_EXE_DEV_KEEP=1` only for the provider process or use a temporary config with keep cleanup.',
    '- On failure after VM creation, attempt cleanup unless preserve mode is requested. Only remove VMs whose names are recorded from this run or clearly use the provider prefix and one of this run\'s sandbox group ids.',
    '- Do not delete unrelated exe.dev VMs.',
    '',
    `Profile: ${profile}`,
    `Provider binary: ${providerBinary}`,
    `Sandbox group id base: ${sandboxGroupId}`,
    `VM count: ${vmCount}`,
    `Workspace marker file: ${workspaceFile}`,
    `Preserve VM: ${preserveVm}`,
    '',
    'Return structured evidence. The vms array must contain one entry for every VM opened, with at least five entries. Keep vmName/sshDest/sessionId/cwd singular fields populated from the first VM for compatibility, but report all VMs in vms. Include every important command in both commandsRun and commandResults. For commandResults, capture exitCode, stdout, stderr, and a short note explaining what the command proved. Truncate very large outputs, but keep enough output to diagnose failures. Do not include secrets.',
  ].join('\n'),
  { phase: 'Real exe.dev E2E', provider: agentProvider, schema: E2E_SCHEMA },
)

phase('Report')
log('Summarizing real exe.dev E2E result')

const final = await agent(
  [
    'Summarize this real exe.dev provider E2E validation.',
    '',
    'Plan:',
    JSON.stringify(plan, null, 2),
    '',
    'E2E result:',
    JSON.stringify(e2e, null, 2),
    '',
    'The summary must be explicit about whether at least five real VMs were created, whether direct SSH/cwd/provider JSONL operations passed on every VM, and whether every VM was deleted or preserved. Include all VMs in the final vms array. Include the most important commandResults in the final result so users can inspect stdout/stderr/exit codes without reading raw logs.',
  ].join('\n'),
  { phase: 'Report', provider: agentProvider, schema: FINAL_SCHEMA },
)

log([
  `exe.dev provider E2E ${final.passed ? 'PASSED' : 'FAILED'}`,
  `summary: ${final.summary}`,
  `vm: ${final.vmName ?? 'none'}`,
  `sshDest: ${final.sshDest ?? 'none'}`,
  final.vms.length
    ? `vms:\n${final.vms.map((vm) => `- ${vm.vmName} (${vm.sshDest}) session=${vm.sessionId} cwd=${vm.cwd} deleted=${vm.deleted} preserved=${vm.preserved}`).join('\n')}`
    : 'vms: none',
  `cleanup: ${final.cleanupOutcome}`,
  final.commandResults.length
    ? `command results:\n${final.commandResults.map((result) => `- ${result.command} => exit=${result.exitCode ?? 'unknown'}; stdout=${JSON.stringify(result.stdout)}; stderr=${JSON.stringify(result.stderr)}; note=${result.note}`).join('\n')}`
    : 'command results: none',
  final.failures.length ? `failures: ${final.failures.join('; ')}` : 'failures: none',
  final.nextSteps.length ? `next steps: ${final.nextSteps.join('; ')}` : 'next steps: none',
].join('\n'))

export default {
  profile,
  preserveVm,
  providerBinary,
  agentProvider,
  sandboxGroupId,
  vmCount,
  plan,
  e2e,
  final,
}
