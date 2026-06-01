import { tool } from '@opencode-ai/plugin'
import { spawn } from 'node:child_process'
import { access, mkdtemp, readFile, writeFile } from 'node:fs/promises'
import { constants } from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const __dirname = path.dirname(fileURLToPath(import.meta.url))
const skillsDir = path.resolve(__dirname, '../plugins/smol-workflows/skills')
const helperPath = path.resolve(skillsDir, 'scripts/smol-wf.sh')

async function exists(file) {
  try {
    await access(file, constants.F_OK)
    return true
  } catch {
    return false
  }
}

function runProcess(command, args, options = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, {
      cwd: options.cwd,
      env: options.env,
      stdio: ['ignore', 'pipe', 'pipe'],
    })

    let stdout = ''
    let stderr = ''

    const abort = () => {
      child.kill('SIGTERM')
    }
    options.signal?.addEventListener('abort', abort, { once: true })

    child.stdout.on('data', (chunk) => {
      stdout += chunk.toString()
    })
    child.stderr.on('data', (chunk) => {
      stderr += chunk.toString()
    })
    child.on('error', reject)
    child.on('close', (code, signal) => {
      options.signal?.removeEventListener('abort', abort)
      resolve({ code, signal, stdout, stderr })
    })
  })
}

function parseJsonObject(text, label) {
  let value
  try {
    value = JSON.parse(text)
  } catch (error) {
    throw new Error(`${label} must be valid JSON: ${error.message}`)
  }
  if (!value || Array.isArray(value) || typeof value !== 'object') {
    throw new Error(`${label} must be a JSON object`)
  }
  return value
}

async function resolveArgsFile(input, context) {
  if (input.argsFile) {
    return path.resolve(context.directory, input.argsFile)
  }

  const argsObject = input.argsJson ? parseJsonObject(input.argsJson, 'argsJson') : {}
  const dir = await mkdtemp(path.join(tmpdir(), 'smol-wf-opencode-'))
  const argsPath = path.join(dir, 'args.json')
  await writeFile(argsPath, `${JSON.stringify(argsObject, null, 2)}\n`, 'utf8')
  return argsPath
}

const listWorkflowsTool = tool({
  description: 'List smol-wf workflows discovered from .agents/workflows and .claude/workflows.',
  args: {},
  async execute(_args, context) {
    const result = await runProcess('bash', [helperPath, 'list'], {
      cwd: context.directory,
      signal: context.abort,
      env: process.env,
    })

    if (result.code !== 0) {
      throw new Error(result.stderr || `smol-wf list failed with exit code ${result.code}`)
    }

    return {
      output: result.stdout.trimEnd() || 'NAME  PATH  DESCRIPTION',
      metadata: {
        stderr: result.stderr,
      },
    }
  },
})

const runWorkflowTool = tool({
  description: 'Run a smol-wf workflow script. Use only when the user explicitly asks to run a workflow.',
  args: {
    path: tool.schema.string().describe('Workflow script path, relative to the project directory or absolute'),
    argsFile: tool.schema.string().optional().describe('Path to a JSON object args file'),
    argsJson: tool.schema.string().optional().describe('Inline JSON object args. Used only when argsFile is not provided'),
    tokenBudget: tool.schema.union([tool.schema.string(), tool.schema.number()]).optional().describe('Output-token budget. Use 0, none, or - to omit'),
    agentProvider: tool.schema.enum(['pi', 'claude-code', 'codex', 'opencode']).optional().describe('Agent provider to pass to smol-wf'),
    maxParallelAgents: tool.schema.number().optional().describe('Concurrency cap; defaults to 4'),
  },
  async execute(input, context) {
    const workflowPath = path.resolve(context.directory, input.path)
    if (!(await exists(workflowPath))) {
      throw new Error(`Workflow script does not exist: ${input.path}`)
    }

    const argsPath = await resolveArgsFile(input, context)
    // Validate early for clearer errors.
    parseJsonObject(await readFile(argsPath, 'utf8'), 'args file')

    const env = {
      ...process.env,
      ...(input.agentProvider ? { SMOL_WF_AGENT_PROVIDER: input.agentProvider } : {}),
      ...(input.maxParallelAgents ? { SMOL_WF_MAX_PARALLEL_AGENTS: String(input.maxParallelAgents) } : {}),
    }

    const result = await runProcess(
      'bash',
      [helperPath, 'run', workflowPath, argsPath, String(input.tokenBudget ?? 0)],
      {
        cwd: context.directory,
        signal: context.abort,
        env,
      },
    )

    if (result.code !== 0) {
      throw new Error(result.stderr || `smol-wf run failed with exit code ${result.code}`)
    }

    let parsed = null
    try {
      parsed = JSON.parse(result.stdout)
    } catch {
      // Keep raw stdout in output when a workflow returns non-JSON unexpectedly.
    }

    return {
      output: result.stdout.trimEnd(),
      metadata: {
        result: parsed,
        stderr: result.stderr,
        workflowPath,
        argsPath,
      },
    }
  },
})

export const SmolWorkflowsPlugin = async () => {
  return {
    config: async (config) => {
      config.skills = config.skills || {}
      config.skills.paths = config.skills.paths || []
      if (!config.skills.paths.includes(skillsDir)) {
        config.skills.paths.push(skillsDir)
      }
    },

    tool: {
      smol_workflows_list: listWorkflowsTool,
      smol_workflows_run: runWorkflowTool,
    },
  }
}

export default SmolWorkflowsPlugin
