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

const schema = schemaPath ? JSON.parse(readFileSync(schemaPath, 'utf8')) : undefined
const output = schema
  ? {
      summary: 'structured debug summary',
      count: 1,
      prompt: stdin,
      required: schema.required ?? [],
    }
  : `fake codex: ${stdin}`

writeFileSync(outputPath, typeof output === 'string' ? output : JSON.stringify(output))
console.log(JSON.stringify({ type: 'turn_complete', usage: { input_tokens: 10, output_tokens: 5, total_tokens: 15 } }))
