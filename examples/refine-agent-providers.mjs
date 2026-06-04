export const meta = {
  name: 'refine-agent-providers-impl',
  description: 'Review and refine agent provider implementations across available providers',
  phases: [
    { title: 'Review', detail: 'Review individual agent provider implementation' },
    { title: 'Synthesize', detail: 'Summarize findings and list action items' },
    { title: 'Update', detail: 'Apply review action items' },
    { title: 'UpdateReview', detail: 'Review generated changes and identify follow-up work' },
  ],
}

const DEFAULT_PROVIDERS = ['opencode', 'pi', 'codex', 'claude-code']
const PROVIDERS = normalizeProviders(args?.providers)
const MIN_UPDATE_PRIORITY = Number(args?.minPriority ?? 4)
const MAX_UPDATE_REVIEW_ITERATIONS = positiveInteger(args?.maxUpdateReviewIterations, 3)

// TODO: Support repeated CLI args soon (e.g. --args-providers opencode --args-providers pi).
// For now, accept either an array from JSON args or a comma-separated string from CLI args.
function positiveInteger(value, fallback) {
  const number = Number(value ?? fallback)
  return Number.isFinite(number) && number > 0 ? Math.floor(number) : fallback
}

function normalizeProviders(value) {
  if (Array.isArray(value)) {
    return value.map(String).filter(Boolean)
  }

  if (typeof value === 'string' && value.trim()) {
    return value.split(',').map(item => item.trim()).filter(Boolean)
  }

  return DEFAULT_PROVIDERS
}

// ── Phase 1: Review provider implementations ─────────────────────────────────
phase('Review')
log(`Reviewing providers: ${PROVIDERS.join(', ')}`)

const REVIEW_SCHEMA = {
  type: 'object',
  properties: {
    feedbacks: {
      type: 'array',
      items: {
        type: 'object',
        properties: {
          file: { type: 'string' },
          description: { type: 'string' },
          priority: { type: 'integer', minimum: 1, maximum: 5 },
        },
        required: ['file', 'description', 'priority'],
      },
    },
  },
  required: ['feedbacks'],
}

const reviewResults = await parallel(
  PROVIDERS.map(name => () =>
    agent(
      `You are a senior Rust code reviewer. Review ./rust/engine/src/agent_providers/${name}.rs.

Focus on correctness, CLI invocation compatibility, structured-output parsing, usage/session parsing, error handling, test coverage, and consistency with the other providers in ./rust/engine/src/agent_providers/.

Return only high-signal findings. Prefer concrete, actionable feedback over style nits.

Return a structured object with:
- feedbacks: array of review feedbacks
  - file: file reviewed
  - description: 2-3 sentence description of the issue and why it matters
  - priority: integer from 1 (least important) to 5 (critical)`,
      {
        label: `review:${name}`,
        phase: 'Review',
        schema: REVIEW_SCHEMA,
      }
    )
  )
)

const providerReviews = reviewResults
  .map((result, index) => ({ provider: PROVIDERS[index], result }))
  .filter(({ result }) => result && Array.isArray(result.feedbacks))

const feedbacks = providerReviews.flatMap(({ provider, result }) =>
  result.feedbacks.map(feedback => ({
    provider,
    file: feedback.file,
    description: feedback.description,
    priority: feedback.priority,
  }))
)

log(`Collected ${feedbacks.length} review feedback item(s)`)

// ── Phase 2: Synthesize findings into an action plan ─────────────────────────
phase('Synthesize')

const SYNTHESIS_SCHEMA = {
  type: 'object',
  properties: {
    summary: { type: 'string' },
    actionItems: {
      type: 'array',
      items: {
        type: 'object',
        properties: {
          provider: { type: 'string' },
          file: { type: 'string' },
          priority: { type: 'integer', minimum: 1, maximum: 5 },
          description: { type: 'string' },
          rationale: { type: 'string' },
          proposedChange: { type: 'string' },
        },
        required: ['provider', 'file', 'priority', 'description', 'rationale', 'proposedChange'],
      },
    },
  },
  required: ['summary', 'actionItems'],
}

const synthesis = await agent(
  `You are a technical lead for this TypeScript workflow engine. Synthesize these agent-provider review findings into a concise implementation action plan.

Deduplicate overlapping findings, discard low-value style-only nits, and keep the plan focused on fixes that improve correctness, compatibility, parsing robustness, usage/session reporting, or test coverage.

Review findings JSON:
${JSON.stringify(feedbacks, null, 2)}

Return a structured object with:
- summary: concise summary of overall provider health and main themes
- actionItems: prioritized concrete changes, each with provider, file, priority, description, rationale, and proposedChange`,
  {
    label: 'synthesize-provider-feedback',
    phase: 'Synthesize',
    schema: SYNTHESIS_SCHEMA,
  }
)

log(`Synthesized ${synthesis.actionItems.length} action item(s)`)

// ── Phase 3 + 4: Apply changes, review them, and feed findings back ─────────
const selectedActionItems = synthesis.actionItems.filter(item => item.priority >= MIN_UPDATE_PRIORITY)
log(`Selected ${selectedActionItems.length} action item(s) with priority >= ${MIN_UPDATE_PRIORITY}`)

const UPDATE_SCHEMA = {
  type: 'object',
  properties: {
    summary: { type: 'string' },
    filesChanged: { type: 'array', items: { type: 'string' } },
    skipped: {
      type: 'array',
      items: {
        type: 'object',
        properties: {
          file: { type: 'string' },
          reason: { type: 'string' },
        },
        required: ['file', 'reason'],
      },
    },
    verification: { type: 'array', items: { type: 'string' } },
  },
  required: ['summary', 'filesChanged', 'skipped', 'verification'],
}

const UPDATE_REVIEW_SCHEMA = {
  type: 'object',
  properties: {
    approved: { type: 'boolean' },
    summary: { type: 'string' },
    findings: {
      type: 'array',
      items: {
        type: 'object',
        properties: {
          file: { type: 'string' },
          description: { type: 'string' },
          severity: { type: 'string', enum: ['low', 'medium', 'high', 'critical'] },
          proposedChange: { type: 'string' },
        },
        required: ['file', 'description', 'severity', 'proposedChange'],
      },
    },
    followUpActions: { type: 'array', items: { type: 'string' } },
  },
  required: ['approved', 'summary', 'findings', 'followUpActions'],
}

function priorityFromSeverity(severity) {
  return {
    low: 2,
    medium: 3,
    high: 4,
    critical: 5,
  }[severity] ?? 3
}

function reviewFindingsToActionItems(findings, iteration) {
  return findings.map((finding, index) => ({
    provider: 'update-review',
    file: finding.file,
    priority: priorityFromSeverity(finding.severity),
    description: finding.description,
    rationale: `Follow-up from UpdateReview iteration ${iteration} with severity ${finding.severity}.`,
    proposedChange: finding.proposedChange,
  }))
}

const skippedUpdate = {
  summary: 'No action items met the update priority threshold.',
  filesChanged: [],
  skipped: [],
  verification: [],
}

const skippedUpdateReview = {
  approved: true,
  summary: 'No generated changes to review.',
  findings: [],
  followUpActions: [],
}

let pendingActionItems = selectedActionItems
const updateIterations = []

if (pendingActionItems.length === 0) {
  phase('Update')
  log('No update action items selected; skipping update/review loop')
  updateIterations.push({
    iteration: 0,
    actionItems: [],
    update: skippedUpdate,
    updateReview: skippedUpdateReview,
  })
}

// TODO: Combine this loop condition with budget checks once the workflow budget global is implemented.
for (let iteration = 1; pendingActionItems.length > 0 && iteration <= MAX_UPDATE_REVIEW_ITERATIONS; iteration += 1) {
  const actionItemsForIteration = pendingActionItems

  phase('Update')
  log(`Update iteration ${iteration}: applying ${actionItemsForIteration.length} action item(s)`)

  const [iterationResult] = await pipeline(
    [actionItemsForIteration],
    actionItems => agent(
      `You are a coding agent working in this repository. Apply the following provider-refinement action items.

Requirements:
- Inspect the target files before changing them.
- Make minimal, focused edits only for the listed action items.
- Add or update tests when the change affects behavior.
- Do not modify unrelated files.
- Run the most relevant typecheck or tests if practical, and report what you ran.
- For every provider affected by the changes, verify the demo workflow with: ./target/debug/smol-wf run ./examples/hello.mjs --agent-provider <provider>
- Reject/revert your own changes for any provider if its hello demo verification fails; report the failure in skipped instead.
- If an item is unsafe, ambiguous, already fixed, cannot be applied, or fails verification, skip it and explain why.

Action items JSON:
${JSON.stringify(actionItems, null, 2)}

Return a structured report with:
- summary: what changed
- filesChanged: files actually modified
- skipped: skipped items with reasons
- verification: commands run or verification performed`,
      {
        label: `update-provider-implementations:${iteration}`,
        phase: 'Update',
        schema: UPDATE_SCHEMA,
      }
    ),
    async update => {
      phase('UpdateReview')
      log(`UpdateReview iteration ${iteration}: reviewing generated changes in ${update.filesChanged.length} file(s)`)

      const updateReview = await agent(
        `You are a senior reviewer. Review the changes generated by the previous update step.

Review scope:
- Confirm the update addressed this iteration's action items.
- Inspect the changed files and relevant tests.
- Look for regressions, incomplete fixes, unsafe edits, unrelated changes, or missing verification.
- Verify that every changed provider was checked with: ./target/debug/smol-wf run ./examples/hello.mjs --agent-provider <provider>
- Reject the update by setting approved: false if any changed provider failed the hello demo verification, or if the update report does not show that this verification was attempted.
- Do not make additional code changes; only review and report.
- If follow-up code changes are required, include them as findings. Leave findings empty when no follow-up changes are needed.

Iteration: ${iteration}
Action items JSON:
${JSON.stringify(actionItemsForIteration, null, 2)}

Update report JSON:
${JSON.stringify(update, null, 2)}

Return a structured review with:
- approved: true only if the generated changes are acceptable as-is
- summary: concise review summary
- findings: concrete follow-up findings with file, description, severity, and proposedChange; use an empty array when no follow-up update is needed
- followUpActions: recommended non-code next steps, if any`,
        {
          label: `review-generated-provider-updates:${iteration}`,
          phase: 'UpdateReview',
          schema: UPDATE_REVIEW_SCHEMA,
        }
      )

      return { iteration, actionItems: actionItemsForIteration, update, updateReview }
    }
  )

  const safeIterationResult = iterationResult ?? {
    iteration,
    actionItems: actionItemsForIteration,
    update: {
      summary: 'Update pipeline returned null.',
      filesChanged: [],
      skipped: actionItemsForIteration.map(item => ({ file: item.file, reason: 'Pipeline item failed.' })),
      verification: [],
    },
    updateReview: {
      approved: false,
      summary: 'Update pipeline returned null before review could complete.',
      findings: [],
      followUpActions: ['Inspect workflow logs and rerun the update pipeline.'],
    },
  }

  updateIterations.push(safeIterationResult)
  pendingActionItems = reviewFindingsToActionItems(safeIterationResult.updateReview.findings, iteration)

  if (pendingActionItems.length === 0) {
    log(`UpdateReview iteration ${iteration}: no follow-up findings remain`)
  } else if (iteration === MAX_UPDATE_REVIEW_ITERATIONS) {
    log(`UpdateReview iteration ${iteration}: stopping with ${pendingActionItems.length} follow-up item(s) after reaching max iterations`)
  } else {
    log(`UpdateReview iteration ${iteration}: feeding ${pendingActionItems.length} follow-up item(s) back to Update`)
  }
}

const finalUpdateIteration = updateIterations[updateIterations.length - 1]
const update = finalUpdateIteration.update
const updateReview = finalUpdateIteration.updateReview

export default {
  providers: PROVIDERS,
  feedbacks,
  synthesis,
  selectedActionItems,
  updateIterations,
  remainingActionItems: pendingActionItems,
  update,
  updateReview,
}
