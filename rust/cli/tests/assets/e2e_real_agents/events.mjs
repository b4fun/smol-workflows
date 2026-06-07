export const meta = {
  name: 'e2e-events',
  description: 'Exercise --events with a nested workflow and one provider call',
}

phase('Prepare event test')
log('event test provider', args.provider)

const child = await workflow(
  { scriptPath: './events-child.mjs' },
  { provider: args.provider },
)

log('event test child complete', args.provider)

export default {
  provider: args.provider,
  child,
}
