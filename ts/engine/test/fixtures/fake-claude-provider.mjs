import { existsSync } from 'node:fs'

const args = process.argv.slice(2)
const schemaIndex = args.indexOf('--json-schema')
const schema = schemaIndex >= 0 ? JSON.parse(args[schemaIndex + 1]) : undefined
const prompt = args[args.length - 1] ?? ''
const projectState = existsSync('.claude') ? 'present' : 'missing'

if (prompt.includes('fail')) {
  console.error('nope')
  process.exit(7)
}

const output = schema
  ? {
      summary: 'structured claude summary',
      count: 2,
      prompt,
      required: schema.required ?? [],
    }
  : projectState === 'present' && !args.includes('--bare')
    ? `fake claude: ${prompt} (project state)`
    : `fake claude: ${prompt}`

console.log(JSON.stringify({
  type: 'result',
  session_id: 'claude-session-1',
  result: typeof output === 'string' ? output : '',
  structured_output: schema ? output : undefined,
  argv: args,
  home: process.env.HOME,
  projectState,
  bareMode: args.includes('--bare'),
  usage: {
    input_tokens: 11,
    output_tokens: 6,
    cache_read_input_tokens: 3,
    cache_creation_input_tokens: 4,
    total_tokens: 24,
  },
  total_cost_usd: 0.123,
}))
