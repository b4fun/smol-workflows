#!/usr/bin/env node
/**
 * Minimal Pi extension + terminating tool structured output demo.
 *
 * This verifies that Pi can load an extension at CLI startup, expose a custom
 * terminating tool, and return the structured data through JSON mode events.
 *
 * It may consume tokens from your configured Pi provider.
 *
 * Usage:
 *   node examples/pi-structured-output-demo.mjs
 *   node examples/pi-structured-output-demo.mjs --model github-copilot/gpt-5.4-mini
 *   PI_MODEL=github-copilot/gpt-5.4-mini node examples/pi-structured-output-demo.mjs
 */

import { spawn } from 'node:child_process'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import process from 'node:process'

const options = parseArgs(process.argv.slice(2))

if (options.help) {
  printHelp()
  process.exit(0)
}

const here = dirname(fileURLToPath(import.meta.url))
const extensionPath = resolve(here, 'pi-structured-output-extension.ts')
const prompt = options.prompt ?? [
  'Use the structured_output tool as your final action exactly once.',
  'Return a nested report proving the Pi extension structured output demo worked.',
  'Include two checks and one recommendation.',
].join('\n')

const args = [
  '--no-extensions',
  '--extension', extensionPath,
  '--no-context-files',
  '--no-skills',
  '--no-prompt-templates',
  '--no-session',
  '--mode', 'json',
  '--print',
  '--tools', 'structured_output',
  ...(options.model ? ['--model', options.model] : []),
  ...(options.provider ? ['--provider', options.provider] : []),
  prompt,
]

const { stdout, stderr, code } = await run('pi', args)
const events = parseJSONLines(stdout)
const structured = extractStructuredOutput(events)
const usage = extractUsage(events)
const eventCounts = countEventTypes(events)
const validation = validateStructuredOutput(structured)

if (code !== 0) {
  console.error(stderr)
  console.error(stdout)
  throw new Error(`pi exited with code ${code}`)
}

console.log(JSON.stringify({
  extensionPath,
  model: options.model,
  provider: options.provider,
  structured,
  validation,
  usage,
  eventCounts,
}, null, 2))

if (!structured || !validation.valid) {
  console.error('Structured output was missing or invalid. Raw stderr/stdout follow.')
  console.error(stderr)
  console.error(stdout)
  process.exitCode = 1
}

function validateStructuredOutput(value) {
  const errors = []

  if (!isNonEmptyObject(value)) {
    return { valid: false, errors: ['structured output is not an object'] }
  }

  if (!isNonEmptyObject(value.report)) errors.push('report must be an object')
  else {
    if (typeof value.report.title !== 'string') errors.push('report.title must be a string')
    if (typeof value.report.status !== 'string') errors.push('report.status must be a string')
    if (typeof value.report.confidence !== 'number') errors.push('report.confidence must be a number')
  }

  if (!Array.isArray(value.checks) || value.checks.length !== 2) errors.push('checks must contain exactly two items')
  else {
    value.checks.forEach((check, index) => {
      if (!isNonEmptyObject(check)) errors.push(`checks[${index}] must be an object`)
      else {
        if (typeof check.id !== 'string') errors.push(`checks[${index}].id must be a string`)
        if (typeof check.passed !== 'boolean') errors.push(`checks[${index}].passed must be a boolean`)
        if (typeof check.evidence !== 'string') errors.push(`checks[${index}].evidence must be a string`)
        if (typeof check.severity !== 'string') errors.push(`checks[${index}].severity must be a string`)
      }
    })
  }

  if (!isNonEmptyObject(value.recommendation)) errors.push('recommendation must be an object')
  else {
    if (typeof value.recommendation.action !== 'string') errors.push('recommendation.action must be a string')
    if (typeof value.recommendation.priority !== 'number') errors.push('recommendation.priority must be a number')
    if (!Array.isArray(value.recommendation.owners) || value.recommendation.owners.length < 1) {
      errors.push('recommendation.owners must be a non-empty array')
    }
  }

  return { valid: errors.length === 0, errors }
}

function run(command, args) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, {
      stdio: ['ignore', 'pipe', 'pipe'],
      env: process.env,
    })

    let stdout = ''
    let stderr = ''

    child.stdout.setEncoding('utf8')
    child.stderr.setEncoding('utf8')
    child.stdout.on('data', chunk => { stdout += chunk })
    child.stderr.on('data', chunk => { stderr += chunk })
    child.on('error', reject)
    child.on('close', code => resolve({ stdout, stderr, code }))
  })
}

function parseJSONLines(text) {
  const events = []

  for (const line of text.split(/\r?\n/)) {
    const trimmed = line.trim()
    if (!trimmed) continue

    try {
      events.push(JSON.parse(trimmed))
    } catch {
      // Ignore non-JSON diagnostics.
    }
  }

  return events
}

function extractStructuredOutput(events) {
  let latest

  for (const event of events) {
    const found = findStructuredOutput(event)
    if (isNonEmptyObject(found)) latest = found
  }

  return latest
}

function findStructuredOutput(value) {
  if (!value || typeof value !== 'object') return undefined

  if (Array.isArray(value)) {
    for (const item of value) {
      const found = findStructuredOutput(item)
      if (found !== undefined) return found
    }
    return undefined
  }

  if (value.type === 'tool_execution_end' && value.toolName === 'structured_output') {
    if (value.result?.details !== undefined) return value.result.details
  }

  if (value.type === 'tool_execution_start' && value.toolName === 'structured_output') {
    if (value.args !== undefined) return value.args
  }

  if (value.toolName === 'structured_output') {
    if (value.result?.details !== undefined) return value.result.details
    if (value.details !== undefined) return value.details
    if (value.args !== undefined) return value.args
  }

  if (value.type === 'toolCall' && value.name === 'structured_output') {
    if (value.arguments !== undefined) return value.arguments
  }

  for (const item of Object.values(value)) {
    const found = findStructuredOutput(item)
    if (found !== undefined) return found
  }

  return undefined
}

function isNonEmptyObject(value) {
  return typeof value === 'object' && value !== null && !Array.isArray(value) && Object.keys(value).length > 0
}

function countEventTypes(events) {
  const counts = {}

  for (const event of events) {
    counts[event.type ?? 'unknown'] = (counts[event.type ?? 'unknown'] ?? 0) + 1
  }

  return counts
}

function extractUsage(events) {
  let latest

  for (const event of events) {
    const usage = findUsage(event)
    if (usage) latest = usage
  }

  return latest
}

function findUsage(value) {
  if (!value || typeof value !== 'object') return undefined

  if (Array.isArray(value)) {
    for (const item of value) {
      const found = findUsage(item)
      if (found) return found
    }
    return undefined
  }

  const maybe = value.usage ?? value.tokens
  if (maybe && typeof maybe === 'object') return maybe

  for (const item of Object.values(value)) {
    const found = findUsage(item)
    if (found) return found
  }

  return undefined
}

function parseArgs(argv) {
  const parsed = {
    model: process.env.PI_MODEL,
    provider: process.env.PI_PROVIDER,
  }

  for (let index = 0; index < argv.length; index++) {
    const arg = argv[index]
    const next = argv[index + 1]

    if (arg === '--help' || arg === '-h') parsed.help = true
    else if (arg === '--model') { parsed.model = requireValue(arg, next); index++ }
    else if (arg === '--provider') { parsed.provider = requireValue(arg, next); index++ }
    else if (arg === '--prompt') { parsed.prompt = requireValue(arg, next); index++ }
    else throw new Error(`Unknown argument: ${arg}`)
  }

  return parsed
}

function requireValue(flag, value) {
  if (!value) throw new Error(`${flag} requires a value`)
  return value
}

function printHelp() {
  console.log(`Usage: node examples/pi-structured-output-demo.mjs [options]\n\nOptions:\n  --model model       Optional Pi model override, e.g. github-copilot/gpt-5.4-mini\n  --provider name     Optional Pi provider override\n  --prompt text       Prompt to send\n  -h, --help          Show this help\n\nEnvironment:\n  PI_MODEL            Same as --model\n  PI_PROVIDER         Same as --provider\n`)
}
