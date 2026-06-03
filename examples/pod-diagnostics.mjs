export const meta = {
  name: 'pod-diagnostics',
  description: 'Diagnose Kubernetes pod status with agent, parallel, and pipeline primitives',
  phases: [
    { title: 'Discover', detail: 'Find the target pods', model: 'github-copilot/gpt-5.4-mini' },
    { title: 'Inspect', detail: 'Gather metrics and logs for each pod', model: 'github-copilot/gpt-5.4-mini' },
    { title: 'Summarize', detail: 'Generate diagnostic guidance', model: 'github-copilot/gpt-5.5' },
  ],
}

const target = typeof args.target === 'string'
  ? args.target
  : 'pods that look unhealthy in the current Kubernetes context'

const POD_LIST_SCHEMA = {
  type: 'object',
  properties: {
    pods: {
      type: 'array',
      items: {
        type: 'object',
        properties: {
          namespace: { type: 'string' },
          name: { type: 'string' },
          reason: { type: 'string' },
        },
        required: ['namespace', 'name', 'reason'],
        additionalProperties: false,
      },
    },
  },
  required: ['pods'],
  additionalProperties: false,
}

phase('Discover')
const { pods } = await agent(
  `List Kubernetes pods to inspect for this request: ${target}. Use kubectl if needed.`,
  { schema: POD_LIST_SCHEMA },
)

phase('Inspect')
const inspections = await parallel(pods.map((pod) => async () => {
  const [inspection] = await pipeline(
    [pod],
    (currentPod) => agent(
      `Get recent metrics and status for pod ${currentPod.namespace}/${currentPod.name}.
Include restarts, readiness, CPU/memory, events, and current phase.`,
      { phase: 'Inspect' },
    ),
    (metrics, currentPod) => agent(
      `Get relevant recent logs for pod ${currentPod.namespace}/${currentPod.name}.
Focus on errors, crashes, probes, and startup failures.
Metrics/status context:
${metrics}`,
      { phase: 'Inspect' },
    ),
  )

  return { pod, inspection }
}))

phase('Summarize')
const diagnostics = await agent(
  `Summarize Kubernetes pod diagnostics for: ${target}
For each pod, identify likely status, evidence, severity, and next actions.
${JSON.stringify(inspections, null, 2)}`,
)

export default { target, pods, inspections, diagnostics }
