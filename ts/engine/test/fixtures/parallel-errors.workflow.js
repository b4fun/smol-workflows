export default await parallel([
  () => agent('ok:first'),
  () => {
    throw new Error('boom')
  },
  async () => {
    throw new Error('async boom')
  },
  () => agent('ok:last'),
])
