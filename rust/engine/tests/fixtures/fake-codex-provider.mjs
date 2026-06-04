import { readFileSync, writeFileSync } from 'node:fs'

const args = process.argv.slice(2)
const outputPath = args[args.indexOf('--output-last-message') + 1]
const schemaIndex = args.indexOf('--output-schema')
const schemaPath = schemaIndex >= 0 ? args[schemaIndex + 1] : undefined
let stdin = ''

process.stdin.setEncoding('utf8')
for await (const chunk of process.stdin) {
  stdin += chunk
}

if (stdin.includes('fail')) {
  process.stderr.write('nope')
  process.exit(7)
}

console.log(JSON.stringify({
  type: 'session_meta',
  payload: { id: 'codex-session-1' },
}))
console.log(JSON.stringify({ type: 'argv', argv: args }))

const schema = schemaPath ? JSON.parse(readFileSync(schemaPath, 'utf8')) : undefined
const output = schema
  ? {
      summary: 'structured debug summary',
      count: 1,
      prompt: stdin,
      required: schema.required ?? [],
      additionalProperties: schema.additionalProperties,
    }
  : `fake codex: ${stdin}`
const finalMessage = schema && stdin.includes('structured-fallback')
  ? `Here is the answer:\n\n${JSON.stringify(output)}`
  : schema && stdin.includes('quoted-structured')
    ? JSON.stringify(JSON.stringify(output))
  : schema && stdin.includes('escaped-structured')
    ? JSON.stringify(output).replace(/"/g, '\\"')
  : typeof output === 'string'
    ? output
    : JSON.stringify(output)
const shouldWriteOutputFile =
  !stdin.includes('stdout-fallback') &&
  !stdin.includes('structured-fallback')

if (shouldWriteOutputFile) {
  writeFileSync(outputPath, finalMessage)
} else {
  console.log(JSON.stringify({
    type: 'message',
    message: { role: 'assistant', content: [{ type: 'text', text: finalMessage }] },
  }))
}
console.log(JSON.stringify({
  type: 'turn_complete',
  usage: stdin.includes('cache-alias')
    ? { input_tokens: 5, output_tokens: 3, cache_read_input_tokens: 4, cache_creation_input_tokens: 2 }
    : { input_tokens: 10, output_tokens: 5, total_tokens: 15 }
}))
