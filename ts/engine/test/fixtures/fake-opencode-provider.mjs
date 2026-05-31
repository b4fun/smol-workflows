const args = process.argv.slice(2)
const prompt = args[args.length - 1]

if (prompt.includes('fail')) {
  console.error('nope')
  process.exit(7)
}

const structured = prompt.includes('Return ONLY valid JSON')
const structuredValue = prompt.includes('escaped-json')
  ? { summary: 'structured opencode summary' }
  : { summary: 'structured opencode summary', prompt }
const structuredOutput = JSON.stringify(structuredValue, null, 2)
const output = structured
  ? (prompt.includes('escaped-json') ? structuredOutput.replace(/\n/g, '\\n').replace(/"/g, '\\"') : structuredOutput)
  : `fake opencode: ${prompt}`

console.log(JSON.stringify({ type: 'message', message: { role: 'assistant', content: [{ type: 'text', text: output }] } }))
console.log(JSON.stringify({ type: 'usage', usage: { input_tokens: 12, output_tokens: 7, total_tokens: 19 } }))
