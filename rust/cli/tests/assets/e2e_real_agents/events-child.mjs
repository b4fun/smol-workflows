export const meta = {
  name: 'e2e-events-child',
  description: 'Nested workflow for --events real-provider e2e',
}

phase('Child event test')
log('child event test provider', args.provider)

const answer = await agent(
  `Reply with exactly one short sentence saying hello from ${args.provider}.`,
  { phase: 'Child event test' },
)

export default {
  provider: args.provider,
  answer,
}
