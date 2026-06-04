export const meta = {
  name: 'example-child-workflow',
  description: 'Child workflow used by examples/workflow-parent.mjs',
  phases: [
    { title: 'Child', detail: 'Handle one item passed by the parent workflow' },
  ],
}

phase('Child')

const item = typeof args.item === 'string' ? args.item : 'unknown item'
log(`Child workflow processing ${item}`)

const summary = await agent(`Summarize this item in one short sentence: ${item}`, {
  phase: 'Child',
})

export default {
  item,
  summary,
}
