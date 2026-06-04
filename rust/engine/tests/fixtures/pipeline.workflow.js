export const meta = {
  name: "pipeline",
  description: "Exercise pipeline stage execution",
  phases: [{ title: "Pipeline" }],
};

export default await pipeline(
  args.items,
  async (item, originalItem, index) => {
    if (item === 'bad') {
      throw new Error('drop bad item')
    }

    return await agent(`stage1:${item}:${originalItem}:${index}`, {
      phase: 'Pipeline',
    })
  },
  async (stage1, originalItem, index) => {
    return await agent(`stage2:${stage1}:${originalItem}:${index}`, {
      phase: 'Pipeline',
    })
  },
)
