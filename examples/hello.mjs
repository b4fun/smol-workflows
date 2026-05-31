export const meta = {
  name: 'hello',
  description: 'Minimal multi-phase smol-workflow example for local and Absurd-backed runs',
  phases: [
    { title: 'Prepare', detail: 'Read workflow args and decide who to greet' },
    { title: 'Draft', detail: 'Create multiple greeting drafts in parallel' },
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

const [friendlyDraft, conciseDraft, enthusiasticDraft] = await parallel([
  () => agent(`Using this plan, write a friendly greeting for ${name}: ${plan}`, {
    key: `draft:friendly:${name}`,
    phase: 'Draft',
  }),
  () => agent(`Using this plan, write a concise greeting for ${name}: ${plan}`, {
    key: `draft:concise:${name}`,
    phase: 'Draft',
  }),
  () => agent(`Using this plan, write an enthusiastic greeting for ${name}: ${plan}`, {
    key: `draft:enthusiastic:${name}`,
    phase: 'Draft',
  }),
])

phase('Finalize')
log(`Finalizing greeting for ${name}`)

const finalGreeting = await agent(
  `Pick the best greeting for ${name} and polish it. Drafts:\n\n${JSON.stringify({
    friendlyDraft,
    conciseDraft,
    enthusiasticDraft,
  }, null, 2)}`,
  {
    key: `final:${name}`,
    phase: 'Finalize',
  },
)

export default {
  name,
  plan,
  drafts: {
    friendly: friendlyDraft,
    concise: conciseDraft,
    enthusiastic: enthusiasticDraft,
  },
  finalGreeting,
}
