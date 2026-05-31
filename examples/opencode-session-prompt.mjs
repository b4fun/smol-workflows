#!/usr/bin/env node
/**
 * Minimal OpenCode server/session prompt demo for structured output.
 *
 * This verifies OpenCode's documented session prompt API:
 *   POST /session/:id/message
 * with:
 *   format: { type: "json_schema", schema, retryCount }
 *
 * It intentionally uses HTTP directly instead of @opencode-ai/sdk so it does not
 * add a repo dependency. It will still invoke your configured OpenCode model and
 * may consume tokens.
 *
 * Usage:
 *   node examples/opencode-session-prompt.mjs
 *   OPENCODE_MODEL=anthropic/claude-sonnet-4-20250514 node examples/opencode-session-prompt.mjs
 *   node examples/opencode-session-prompt.mjs --model anthropic/claude-sonnet-4-20250514 --agent build
 */

import { spawn } from 'node:child_process'
import process from 'node:process'

const options = parseArgs(process.argv.slice(2))

if (options.help) {
  printHelp()
  process.exit(0)
}

const server = await startOpenCodeServer()

try {
  const session = await request(server.url, '/session', {
    method: 'POST',
    query: { directory: process.cwd() },
    body: {
      title: 'smol-workflows structured output demo',
      ...(options.agent ? { agent: options.agent } : {}),
    },
  })

  const sessionID = session.id

  if (typeof sessionID !== 'string') {
    throw new Error(`OpenCode create-session response did not include a string id: ${JSON.stringify(session)}`)
  }

  const schema = options.complex ? complexSchema() : simpleSchema()

  const promptResponse = await request(server.url, `/session/${encodeURIComponent(sessionID)}/message`, {
    method: 'POST',
    query: { directory: process.cwd() },
    body: {
      ...(options.model ? { model: splitModel(options.model) } : {}),
      ...(options.agent ? { agent: options.agent } : {}),
      parts: [
        {
          type: 'text',
          text: options.prompt ?? (options.complex
            ? 'Analyze the structured-output demo and return a nested report with two checks and one recommendation.'
            : 'Return a tiny structured report about why JSON Schema output is useful.'),
        },
      ],
      format: {
        type: 'json_schema',
        schema,
        retryCount: options.retryCount,
      },
    },
  })

  const structured = extractStructuredOutput(promptResponse)

  console.log(JSON.stringify({
    server: server.url,
    sessionID,
    structured,
    response: promptResponse,
  }, null, 2))
} finally {
  server.stop()
}

async function startOpenCodeServer() {
  const child = spawn('opencode', ['serve', '--hostname', '127.0.0.1', '--port', '0', '--pure'], {
    stdio: ['ignore', 'pipe', 'pipe'],
    env: process.env,
  })

  let logs = ''
  let settled = false

  const url = await new Promise((resolve, reject) => {
    const timeout = setTimeout(() => {
      reject(new Error(`Timed out waiting for opencode serve URL. Logs:\n${logs}`))
    }, 15_000)

    child.on('error', reject)
    child.on('exit', (code, signal) => {
      if (!settled) {
        clearTimeout(timeout)
        reject(new Error(`opencode serve exited early (${signal ?? code}). Logs:\n${logs}`))
      }
    })

    for (const stream of [child.stdout, child.stderr]) {
      stream.setEncoding('utf8')
      stream.on('data', (chunk) => {
        logs += chunk
        const match = logs.match(/opencode server listening on (http:\/\/[^\s]+)/)
        if (match?.[1] && !settled) {
          settled = true
          clearTimeout(timeout)
          resolve(match[1])
        }
      })
    }
  })

  return {
    url,
    stop() {
      child.kill('SIGTERM')
    },
  }
}

async function request(baseUrl, path, { method, query, body }) {
  const url = new URL(path, baseUrl)

  for (const [key, value] of Object.entries(query ?? {})) {
    if (value !== undefined) url.searchParams.set(key, String(value))
  }

  const response = await fetch(url, {
    method,
    headers: { 'content-type': 'application/json' },
    body: body === undefined ? undefined : JSON.stringify(body),
  })

  const text = await response.text()
  const data = text ? JSON.parse(text) : undefined

  if (!response.ok) {
    throw new Error(`${method} ${url} failed with ${response.status}: ${text}`)
  }

  return data
}

function extractStructuredOutput(value) {
  if (!value || typeof value !== 'object') return undefined

  if (Array.isArray(value)) {
    for (const item of value) {
      const found = extractStructuredOutput(item)
      if (found !== undefined) return found
    }
    return undefined
  }

  if (Object.hasOwn(value, 'structured_output')) return value.structured_output
  if (Object.hasOwn(value, 'structuredOutput')) return value.structuredOutput
  if (Object.hasOwn(value, 'structured')) return value.structured

  if (value.type === 'tool' && value.tool === 'StructuredOutput') {
    const input = value.state?.input
    if (input !== undefined) return input
  }

  for (const item of Object.values(value)) {
    const found = extractStructuredOutput(item)
    if (found !== undefined) return found
  }

  return undefined
}

function simpleSchema() {
  return {
    type: 'object',
    properties: {
      summary: {
        type: 'string',
        description: 'One short sentence proving structured output worked.',
      },
      confidence: {
        type: 'number',
        minimum: 0,
        maximum: 1,
        description: 'Confidence from 0 to 1.',
      },
      tags: {
        type: 'array',
        items: { type: 'string' },
        description: 'Two or three short tags.',
      },
    },
    required: ['summary', 'confidence', 'tags'],
    additionalProperties: false,
  }
}

function complexSchema() {
  return {
    type: 'object',
    properties: {
      report: {
        type: 'object',
        properties: {
          title: { type: 'string' },
          status: { type: 'string', enum: ['pass', 'warn', 'fail'] },
          confidence: { type: 'number', minimum: 0, maximum: 1 },
        },
        required: ['title', 'status', 'confidence'],
        additionalProperties: false,
      },
      checks: {
        type: 'array',
        minItems: 2,
        maxItems: 2,
        items: {
          type: 'object',
          properties: {
            id: { type: 'string' },
            passed: { type: 'boolean' },
            evidence: { type: 'string' },
            severity: { type: 'string', enum: ['low', 'medium', 'high'] },
          },
          required: ['id', 'passed', 'evidence', 'severity'],
          additionalProperties: false,
        },
      },
      recommendation: {
        type: 'object',
        properties: {
          action: { type: 'string' },
          priority: { type: 'integer', minimum: 1, maximum: 5 },
          owners: {
            type: 'array',
            items: { type: 'string' },
            minItems: 1,
          },
        },
        required: ['action', 'priority', 'owners'],
        additionalProperties: false,
      },
    },
    required: ['report', 'checks', 'recommendation'],
    additionalProperties: false,
  }
}

function splitModel(model) {
  const index = model.indexOf('/')

  if (index <= 0 || index === model.length - 1) {
    throw new Error(`--model must use provider/model form, got: ${model}`)
  }

  return {
    providerID: model.slice(0, index),
    modelID: model.slice(index + 1),
  }
}

function parseArgs(argv) {
  const parsed = {
    model: process.env.OPENCODE_MODEL,
    agent: process.env.OPENCODE_AGENT,
    retryCount: Number(process.env.OPENCODE_RETRY_COUNT ?? 2),
  }

  for (let index = 0; index < argv.length; index++) {
    const arg = argv[index]
    const next = argv[index + 1]

    if (arg === '--help' || arg === '-h') parsed.help = true
    else if (arg === '--model') { parsed.model = requireValue(arg, next); index++ }
    else if (arg === '--agent') { parsed.agent = requireValue(arg, next); index++ }
    else if (arg === '--retry-count') { parsed.retryCount = Number(requireValue(arg, next)); index++ }
    else if (arg === '--prompt') { parsed.prompt = requireValue(arg, next); index++ }
    else if (arg === '--complex') parsed.complex = true
    else throw new Error(`Unknown argument: ${arg}`)
  }

  if (!Number.isInteger(parsed.retryCount) || parsed.retryCount < 0) {
    throw new Error(`retryCount must be a non-negative integer, got: ${parsed.retryCount}`)
  }

  return parsed
}

function requireValue(flag, value) {
  if (!value) throw new Error(`${flag} requires a value`)
  return value
}

function printHelp() {
  console.log(`Usage: node examples/opencode-session-prompt.mjs [options]\n\nOptions:\n  --model provider/model   Optional OpenCode model override\n  --agent name             Optional OpenCode agent override\n  --retry-count n          Structured-output retry count (default: 2)\n  --prompt text            Prompt to send\n  --complex                Use a more complex nested JSON Schema\n  -h, --help               Show this help\n\nEnvironment:\n  OPENCODE_MODEL           Same as --model\n  OPENCODE_AGENT           Same as --agent\n  OPENCODE_RETRY_COUNT     Same as --retry-count\n`)
}
