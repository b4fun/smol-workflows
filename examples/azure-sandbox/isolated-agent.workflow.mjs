/** @type {import('@smol-workflows/sdk').WorkflowMetadata} */
export const meta = {
  name: 'azure-sandbox-isolated-agent',
  description: 'Minimal workflow that opens an Azure Sandbox profile for one agent call.',
}

export default await agent(
  'Say hello from an agent running with Azure Sandbox isolation. Keep it to one sentence.',
  {
    isolation: { type: 'sandbox', profile: 'azure-sandbox/default' },
  },
)
