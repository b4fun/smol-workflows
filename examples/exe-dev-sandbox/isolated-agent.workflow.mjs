/** @type {import('@smol-workflows/sdk').WorkflowMetadata} */
export const meta = {
  name: 'exe-dev-sandbox-isolated-agent',
  description: 'Open an exe.dev VM sandbox and ask one agent call to run a small smoke test inside it.',
  whenToUse: 'Use to verify that smol-workflows can create an exe.dev sandbox and route an agent call through it.',
  phases: [
    { title: 'Sandbox smoke test', detail: 'Run one isolated agent call using exe-dev/default' },
  ],
}

const profile = typeof args.profile === 'string' ? args.profile : 'exe-dev/default'
const markerFile = typeof args.markerFile === 'string'
  ? args.markerFile
  : '.smol-exe-dev-smoke.txt'

/** @satisfies {import('@smol-workflows/sdk').JSONSchema} */
const SMOKE_REPORT_SCHEMA = {
  type: 'object',
  properties: {
    status: {
      type: 'string',
      enum: ['ok', 'failed', 'unknown'],
    },
    cwd: { type: 'string' },
    uname: { type: 'string' },
    markerFile: { type: 'string' },
    markerContent: { type: 'string' },
    observations: {
      type: 'array',
      items: { type: 'string' },
    },
  },
  required: ['status', 'cwd', 'uname', 'markerFile', 'markerContent', 'observations'],
  additionalProperties: false,
}

phase('Sandbox smoke test')
log(`Opening sandbox profile ${profile}`)

const report = await agent(
  [
    'You are running inside an exe.dev sandbox VM opened by smol-workflows.',
    'Run a short smoke test using shell commands, then return the structured report.',
    '',
    'Required checks:',
    '1. Print the current working directory with `pwd`.',
    '2. Print OS/kernel information with `uname -a`.',
    `3. Write the text "hello from exe.dev sandbox" to ${markerFile}.`,
    `4. Read ${markerFile} back and report its content.`,
    '5. Mention whether the workspace appears to be synced.',
    '',
    'Do not make unrelated changes and do not commit anything.',
  ].join('\n'),
  {
    phase: 'Sandbox smoke test',
    isolation: { type: 'sandbox', profile },
    schema: SMOKE_REPORT_SCHEMA,
  },
)

export default {
  profile,
  markerFile,
  report,
}
