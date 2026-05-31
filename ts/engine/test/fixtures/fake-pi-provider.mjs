const args = process.argv.slice(2)
const prompt = args[args.length - 1]

if (prompt.includes('fail')) {
  console.error('nope')
  process.exit(7)
}

const structured = prompt.includes('Return ONLY valid JSON')
const output = structured
  ? JSON.stringify({ summary: 'structured pi summary', prompt })
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
console.log(JSON.stringify({ type: 'message_end', message: { role: 'assistant', content: [{ type: 'text', text: output }], usage } }))
console.log(JSON.stringify({ type: 'agent_end', messages: [{ role: 'assistant', content: [{ type: 'text', text: output }], usage }] }))
