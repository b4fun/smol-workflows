export const meta = {
  name: 'example-parent-workflow',
  description: 'Demonstrates calling a child workflow with workflow({ scriptPath }, args)',
  phases: [
    { title: 'Prepare', detail: 'Choose items for child workflow calls' },
    { title: 'Children', detail: 'Run child workflows inline' },
    { title: 'Synthesize', detail: 'Combine child workflow outputs' },
  ],
}

phase('Prepare')

const items = Array.isArray(args.items) && args.items.length
  ? args.items.map(String)
  : ['alpha', 'beta']

log(`Parent workflow will process: ${items.join(', ')}`)

phase('Children')

const childResults = await parallel(
  items.map(item => () => workflow(
    { scriptPath: './workflow-child.mjs' },
    { item },
  )),
)

phase('Synthesize')

const synthesis = await agent(
  `Combine these child workflow results into a concise final report:\n\n${JSON.stringify(childResults, null, 2)}`,
  {
    key: `parent-synthesis:${items.join(',')}`,
    phase: 'Synthesize',
  },
)

export default {
  items,
  childResults,
  synthesis,
}
