export const meta = { name: 'isolated-agent', description: 'isolation test' }
export default await agent('touch isolated workspace', { isolation: 'worktree' })
