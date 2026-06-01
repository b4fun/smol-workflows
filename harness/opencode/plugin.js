import path from 'node:path'
import { fileURLToPath } from 'node:url'

const __dirname = path.dirname(fileURLToPath(import.meta.url))
const skillsDir = path.resolve(__dirname, 'plugins/smol-workflows/skills')

const bootstrapText = `<smol-workflows>
smol-workflows skills are available in this session.

Use OpenCode's skill tool to load:
- smol-workflows/list when the user asks to list or inspect workflows
- smol-workflows/create when the user asks to create or edit a workflow
- smol-workflows/run when the user asks to run an existing workflow

Only use smol-workflows after explicit user opt-in to workflows, smol-wf, or multi-agent orchestration.
</smol-workflows>`

export const SmolWorkflowsPlugin = async () => {
  return {
    config: async (config) => {
      config.skills = config.skills || {}
      config.skills.paths = config.skills.paths || []
      if (!config.skills.paths.includes(skillsDir)) {
        config.skills.paths.push(skillsDir)
      }
    },

    'experimental.chat.messages.transform': async (_input, output) => {
      if (!output.messages?.length) return
      const firstUser = output.messages.find((message) => message.info.role === 'user')
      if (!firstUser?.parts?.length) return
      if (firstUser.parts.some((part) => part.type === 'text' && part.text.includes('<smol-workflows>'))) return

      const referencePart = firstUser.parts[0]
      firstUser.parts.unshift({
        ...referencePart,
        type: 'text',
        text: bootstrapText,
      })
    },
  }
}

export default SmolWorkflowsPlugin
