export const meta = {
  name: 'hello',
  description: 'Minimal multi-phase smol-workflow example for local and Absurd-backed runs',
  phases: [
    { title: 'Prepare', detail: 'Read workflow args and decide who to greet' },
    { title: 'Draft', detail: 'Create multiple greeting drafts with pipeline' },
    { title: 'Finalize', detail: 'Pick and polish the final greeting' },
  ],
}

phase('Prepare')

const name = typeof args.name === 'string' ? args.name : 'world'
log(`Preparing greeting workflow for ${name}`)

const plan = await agent(`Create a short greeting plan for ${name}`, {
  key: `plan:${name}`,
  phase: 'Prepare',
})

phase('Draft')
log(`Creating greeting drafts for ${name}`)

const draftStyles = ['friendly', 'concise', 'enthusiastic']

const draftResults = await pipeline(
  draftStyles,
  (style) => agent(`Using this plan, write a ${style} greeting for ${name}: ${plan}`, {
    key: `draft:${style}:${name}`,
    phase: 'Draft',
  }),
  (draft, style) => agent(`Improve this ${style} greeting for ${name}: ${draft}`, {
    key: `improve:${style}:${name}`,
    phase: 'Draft',
  }),
)

const drafts = Object.fromEntries(
  draftStyles.map((style, index) => [style, draftResults[index]]),
)

phase('Finalize')
log(`Finalizing greeting for ${name}`)

const finalGreeting = await agent(
  `Pick the best greeting for ${name} and polish it. Drafts:\n\n${JSON.stringify(drafts, null, 2)}`,
  {
    key: `final:${name}`,
    phase: 'Finalize',
  },
)

export default {
  name,
  plan,
  drafts,
  finalGreeting,
  budget: {
    total: budget.total,
    spent: budget.spent(),
    remaining: budget.total === null ? null : budget.remaining(),
  },
}
