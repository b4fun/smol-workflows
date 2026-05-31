import { mkdirSync, writeFileSync } from 'node:fs'

const args = process.argv.slice(2)
const outputPath = args[args.indexOf('--output-last-message') + 1]
let stdin = ''

process.stdin.setEncoding('utf8')
for await (const chunk of process.stdin) {
  stdin += chunk
}

// Create a *directory* at the output path so readFile() fails with EISDIR (not ENOENT).
mkdirSync(outputPath, { recursive: true })

console.log(JSON.stringify({ type: 'turn_complete', usage: { input_tokens: 1, output_tokens: 1, total_tokens: 2 } }))
