import { readFileSync } from 'node:fs'

const args = process.argv.slice(2)
const prompt = args[args.length - 1]

if (prompt.includes('fail')) {
  console.error('nope')
  process.exit(7)
}

const extensionIndex = args.indexOf('--extension')
const extensionPath = extensionIndex >= 0 ? args[extensionIndex + 1] : undefined
const extensionSource = extensionPath ? readFileSync(extensionPath, 'utf8') : ''
const structured = extensionSource.includes('smol_workflows_structured_output')
const output = structured
  ? JSON.stringify({ summary: 'structured pi summary', prompt, extensionRegisteredTool: extensionSource.includes('pi.registerTool(structuredOutputTool)') })
  : `fake pi: ${prompt}`
const usage = {
  input: 13,
  output: 8,
  cacheRead: 2,
  cacheWrite: 3,
  totalTokens: 26,
  cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
}

console.log(JSON.stringify({ type: 'session', id: 'pi-session-1' }))
if (structured) {
  const details = JSON.parse(output)
  console.log(JSON.stringify({ type: 'tool_execution_start', toolName: 'smol_workflows_structured_output', toolCallId: 'call-1', args: details }))
  console.log(JSON.stringify({
    type: 'tool_execution_end',
    toolName: 'smol_workflows_structured_output',
    toolCallId: 'call-1',
    result: {
      content: [{ type: 'text', text: 'Structured output captured successfully.' }],
      details,
      terminate: true,
    },
    isError: false,
  }))
} else {
  console.log(JSON.stringify({ type: 'message_end', message: { role: 'assistant', content: [{ type: 'text', text: output }], usage } }))
  console.log(JSON.stringify({ type: 'agent_end', messages: [{ role: 'assistant', content: [{ type: 'text', text: output }], usage }] }))
}
