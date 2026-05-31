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

if (prompt.includes('code-fence')) {
  // Return JSON wrapped in a markdown code fence.
  const fenced = '```json\n' + structuredOutput + '\n```'
  console.log(JSON.stringify({ type: 'message', message: { role: 'assistant', content: [{ type: 'text', text: fenced }] } }))
  console.log(JSON.stringify({ type: 'usage', usage: { input_tokens: 1, output_tokens: 1, total_tokens: 2 } }))
  process.exit(0)
}

if (prompt.includes('usage-nested')) {
  // Return usage inside a nested structure — must not be double-counted.
  console.log(JSON.stringify({
    type: 'usage',
    data: {
      usage: { input_tokens: 5, output_tokens: 3, total_tokens: 8 }
    }
  }))
  console.log(JSON.stringify({ type: 'message', message: { role: 'assistant', content: [{ type: 'text', text: 'nested usage result' }] } }))
  process.exit(0)
}

if (prompt.includes('cache-alias')) {
  // Return usage with cache sub-object (opencode normalizeUsage reads cache.read / cache.write).
  console.log(JSON.stringify({
    type: 'message', message: { role: 'assistant', content: [{ type: 'text', text: 'cache alias result' }] }
  }))
  console.log(JSON.stringify({
    type: 'usage',
    usage: {
      input_tokens: 10,
      output_tokens: 4,
      cache: { read: 2, write: 3 },
    }
  }))
  process.exit(0)
}

if (prompt.includes('tool-use-alongside-text')) {
  // Emit a message with both a tool_use block and a text block.
  // The provider must return the text string, NOT the tool_use object.
  console.log(JSON.stringify({
    type: 'message',
    message: {
      role: 'assistant',
      content: [
        { type: 'tool_use', id: 'tu_1', name: 'read_file', input: { path: '/tmp/x' } },
        { type: 'text', text: 'tool use result text' },
      ],
    },
  }))
  console.log(JSON.stringify({ type: 'usage', usage: { input_tokens: 3, output_tokens: 2, total_tokens: 5 } }))
  process.exit(0)
}

const output = structured
  ? (prompt.includes('escaped-json') ? structuredOutput.replace(/\n/g, '\\n').replace(/"/g, '\\"') : structuredOutput)
  : `fake opencode: ${prompt}`

console.log(JSON.stringify({ type: 'message', message: { role: 'assistant', content: [{ type: 'text', text: output }] } }))
console.log(JSON.stringify({ type: 'usage', usage: { input_tokens: 12, output_tokens: 7, total_tokens: 19 } }))
