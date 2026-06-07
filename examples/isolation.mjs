/** @type {import('@smol-workflows/sdk').WorkflowMetadata} */
export const meta = {
  name: 'isolation-demo',
  description: 'Run file-mutating agent experiments in temporary git worktrees',
  whenToUse: 'Use when multiple agents should explore code changes without touching the caller worktree.',
  phases: [
    { title: 'Plan', detail: 'Describe the target change' },
    { title: 'Explore', detail: 'Run isolated agents in separate temporary git worktrees' },
    { title: 'Synthesize', detail: 'Compare isolated proposals and recommend next steps' },
  ],
}

// Paths are resolved from this workflow file's directory (`examples/` when run from this repo).
const target = typeof args.target === 'string' ? args.target : '../README.md'
const goal = typeof args.goal === 'string'
  ? args.goal
  : 'make the documentation clearer for a first-time user'

/** @satisfies {import('@smol-workflows/sdk').JSONSchema} */
const PROPOSAL_SCHEMA = {
  type: 'object',
  properties: {
    approach: { type: 'string' },
    changedFiles: {
      type: 'array',
      items: { type: 'string' },
    },
    diff: { type: 'string' },
    notes: { type: 'string' },
  },
  required: ['approach', 'changedFiles', 'diff'],
}

phase('Plan')
log(`Preparing isolated change experiments for ${target}`)

const plan = await agent(
  `We want to improve ${target}. Goal: ${goal}.\n\nCreate a brief implementation plan.`,
  { phase: 'Plan' },
)

phase('Explore')
log('Starting isolated agents. Each agent gets its own temporary git worktree.')

const variants = [
  {
    name: 'minimal',
    instruction: 'Make the smallest useful edit that satisfies the goal.',
  },
  {
    name: 'ambitious',
    instruction: 'Try a more comprehensive edit while keeping the file coherent.',
  },
]

const proposals = await parallel(variants.map((variant) => async () => {
  const proposal = await agent(
    [
      `You are running in a temporary isolated git worktree created for this single agent call.`,
      `Target file: ${target}`,
      `Goal: ${goal}`,
      `Plan: ${plan}`,
      `Variant: ${variant.name}`,
      variant.instruction,
      '',
      'If appropriate, edit files in this isolated worktree. Do not commit.',
      'Before finishing, inspect `git diff --stat` and `git diff`.',
      'Return the approach, changed files, diff, and any notes.',
    ].join('\n'),
    {
      phase: 'Explore',
      isolation: 'worktree',
      schema: PROPOSAL_SCHEMA,
    },
  )
  return { variant: variant.name, proposal }
}))

phase('Synthesize')
log('Synthesizing isolated proposals')

const recommendation = await agent(
  [
    `Compare these isolated worktree proposals for ${target}.`,
    `Goal: ${goal}`,
    '',
    JSON.stringify(proposals, null, 2),
    '',
    'Recommend which proposal to apply, or describe a merged approach.',
    'Remember: the isolated worktrees were temporary; only the returned diffs/reports remain.',
  ].join('\n'),
  { phase: 'Synthesize' },
)

export default {
  target,
  goal,
  plan,
  proposals,
  recommendation,
}
