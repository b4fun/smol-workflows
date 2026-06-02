export const meta = {
  name: 'security-review',
  description: 'Discover current code changes and request concise security review feedback',
  phases: [
    { title: 'Discover', detail: 'Inspect current repository changes' },
    { title: 'Review', detail: 'Assess security implications of the changes' },
    { title: 'Refute', detail: 'Challenge findings and keep only actionable issues' },
  ],
}

const REVIEW_SCHEMA = {
  type: 'object',
  properties: {
    items: {
      type: 'array',
      items: {
        type: 'object',
        properties: {
          target: {
            type: 'string',
            description: 'File, function, behavior, or change being reviewed',
          },
          rating: {
            type: 'string',
            enum: ['low', 'medium', 'high', 'critical'],
            description: 'Security severity rating',
          },
          suggestion: {
            type: 'string',
            description: 'Concrete recommendation for addressing the issue',
          },
        },
        required: ['target', 'rating', 'suggestion'],
      },
    },
  },
  required: ['items'],
}

const REFUTE_SCHEMA = {
  type: 'object',
  properties: {
    refuted: {
      type: 'boolean',
      description: 'True when the finding is not security-relevant, not supported by the discovered changes, or not actionable',
    },
    reason: {
      type: 'string',
      description: 'Concise explanation for the verdict',
    },
  },
  required: ['refuted', 'reason'],
}

phase('Discover')
log('Discovering current repository changes for security review')

const scope = typeof args.scope === 'string' && args.scope.trim()
  ? args.scope.trim()
  : 'the current working tree changes, including staged and unstaged changes'

const discoverPrompt = [
  `Inspect ${scope}.`,
  'Do not edit files or make changes in this codebase; only review and report.',
  'Summarize the changed files and the security-relevant behavior changes.',
  'Focus on what changed, not on giving final recommendations yet.',
].join(' ')

const changes = await agent(
  discoverPrompt,
  {
    phase: 'Discover',
  },
)

phase('Review')
log('Requesting security review feedback')

const reviewPrompt = [
  'Provide concise security review feedback for these discovered changes.',
  'Do not edit files or make changes in this codebase; only return review findings.',
  'Return only findings that are concrete, security-relevant, and actionable.',
  'For each finding include target, rating, and suggestion.',
  'If there are no meaningful security concerns, return an empty items array.',
  '',
  'Discovered changes:',
  changes,
].join('\n')

const review = await agent(
  reviewPrompt,
  {
    phase: 'Review',
    schema: REVIEW_SCHEMA,
  },
)

const findings = Array.isArray(review.items) ? review.items : []

phase('Refute')
log(`Refuting ${findings.length} security review finding(s)`)

const verdicts = await parallel(findings.map((finding, index) => () => agent(
  [
    'Try to refute this security review finding.',
    'Do not edit files or make changes in this codebase; only judge the finding.',
    'Mark refuted=true if the finding is not supported by the discovered changes, is not security-relevant, is too speculative, or is not actionable.',
    'Mark refuted=false only if it should be addressed.',
    '',
    'Discovered changes:',
    changes,
    '',
    'Finding:',
    JSON.stringify(finding, null, 2),
  ].join('\n'),
  {
    phase: 'Refute',
    schema: REFUTE_SCHEMA,
  },
)))

const itemsToAddress = findings.filter((_, index) => {
  const verdict = verdicts[index]
  return verdict && verdict.refuted === false
})

export default {
  scope,
  changes,
  review: {
    items: findings,
  },
  refutations: verdicts,
  itemsToAddress,
}
