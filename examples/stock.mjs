/** @type {import('@smol-workflows/sdk').WorkflowMetadata} */
export const meta = {
  name: 'stock-investment-analysis',
  description: 'Three-phase stock investment analysis: decompose → research → synthesize',
  phases: [
    { title: 'Analyze', detail: 'Decompose investment question into research dimensions' },
    { title: 'Research', detail: 'Parallel agents research each stock across multiple dimensions' },
    { title: 'Synthesize', detail: 'Summarize all findings into actionable investment insights' },
  ],
}

const STOCKS = Array.isArray(args.stocks) ? args.stocks.map(String) : ['NVDA', 'SPCE']

// ── Phase 1: Analyze the ask ──────────────────────────────────────────────────
phase('Analyze')
log(`Decomposing investment analysis for: ${STOCKS.join(', ')}`)

/** @satisfies {import('@smol-workflows/sdk').JSONSchema} */
const DIMENSION_SCHEMA = {
  type: 'object',
  properties: {
    dimensions: {
      type: 'array',
      items: {
        type: 'object',
        properties: {
          key: { type: 'string' },
          label: { type: 'string' },
          prompt: { type: 'string' },
        },
        required: ['key', 'label', 'prompt'],
      },
    },
    investorContext: { type: 'string' },
  },
  required: ['dimensions', 'investorContext'],
}

const decomposition = /** @type {{ investorContext: string, dimensions: Array<{ key: string, label: string, prompt: string }> }} */ (await agent(
  `You are a senior equity research strategist. The user wants to analyze these stocks for potential investment: ${STOCKS.join(', ')}.

Decompose this into 5 research dimensions that would give a comprehensive investment picture. For each dimension, write a specific research prompt that a financial analyst agent should answer using web search.

Focus on dimensions like: fundamentals, growth catalysts, risks, valuation, and market sentiment/technicals.

Return a JSON object with:
- dimensions: array of {key, label, prompt} — 5 dimensions total
- investorContext: a 2-sentence framing of what kind of investor would be interested in these two stocks together and what the key comparison tension is`,
  { schema: DIMENSION_SCHEMA, phase: 'Analyze' }
) ?? { investorContext: '', dimensions: [] })

log(`Investor context: ${decomposition.investorContext}`)
log(`Research dimensions: ${decomposition.dimensions.map(d => d.label).join(', ')}`)

// ── Phase 2: Parallel research agents ─────────────────────────────────────────
phase('Research')
log(`Spawning ${STOCKS.length * decomposition.dimensions.length} research agents (${STOCKS.length} stocks × ${decomposition.dimensions.length} dimensions)`)

/** @satisfies {import('@smol-workflows/sdk').JSONSchema} */
const FINDING_SCHEMA = {
  type: 'object',
  properties: {
    stock: { type: 'string' },
    dimension: { type: 'string' },
    summary: { type: 'string' },
    keyPoints: { type: 'array', items: { type: 'string' } },
    signal: { type: 'string', enum: ['bullish', 'bearish', 'neutral', 'mixed'] },
    confidence: { type: 'string', enum: ['high', 'medium', 'low'] },
  },
  required: ['stock', 'dimension', 'summary', 'keyPoints', 'signal', 'confidence'],
}

const researchTasks = []
for (const stock of STOCKS) {
  for (const dim of decomposition.dimensions) {
    researchTasks.push({ stock, dim })
  }
}

const findings = await parallel(
  researchTasks.map(({ stock, dim }) => () =>
    agent(
      `You are a financial analyst. Research the following about ${stock} stock:

${dim.prompt}

Use your knowledge up to your training cutoff (August 2025) to provide a thorough, factual analysis. Be specific with numbers, dates, and data points where possible.

Return a structured finding with:
- stock: "${stock}"
- dimension: "${dim.label}"
- summary: 2-3 sentence summary of findings
- keyPoints: 4-6 specific, data-backed bullet points
- signal: your overall signal for this dimension (bullish/bearish/neutral/mixed)
- confidence: your confidence level (high/medium/low) based on data availability`,
      {
        label: `${stock}:${dim.key}`,
        phase: 'Research',
        schema: FINDING_SCHEMA,
      }
    )
  )
)

const validFindings = findings.filter(Boolean)
log(`Collected ${validFindings.length} research findings`)

// ── Phase 3: Synthesize ───────────────────────────────────────────────────────
phase('Synthesize')

/** @satisfies {import('@smol-workflows/sdk').JSONSchema} */
const SYNTHESIS_SCHEMA = {
  type: 'object',
  properties: {
    executiveSummary: { type: 'string' },
    stockAnalyses: {
      type: 'array',
      items: {
        type: 'object',
        properties: {
          ticker: { type: 'string' },
          overallSignal: { type: 'string', enum: ['Strong Buy', 'Buy', 'Hold', 'Sell', 'Strong Sell'] },
          thesis: { type: 'string' },
          bullCase: { type: 'string' },
          bearCase: { type: 'string' },
          keyRisks: { type: 'array', items: { type: 'string' } },
          suitableFor: { type: 'string' },
        },
        required: ['ticker', 'overallSignal', 'thesis', 'bullCase', 'bearCase', 'keyRisks', 'suitableFor'],
      },
    },
    comparison: { type: 'string' },
    portfolioRecommendation: { type: 'string' },
    disclaimer: { type: 'string' },
  },
  required: ['executiveSummary', 'stockAnalyses', 'comparison', 'portfolioRecommendation', 'disclaimer'],
}

const synthesis = await agent(
  `You are a chief investment officer synthesizing research from your analyst team. Here are all the research findings:

${JSON.stringify(validFindings, null, 2)}

Investor context: ${decomposition.investorContext}

Synthesize these findings into a comprehensive investment report. Be direct, opinionated, and actionable. Include:
- executiveSummary: 3-4 sentence high-level takeaway
- stockAnalyses: one entry per stock with overall signal, thesis, bull/bear cases, key risks, and who it's suitable for
- comparison: 2-3 sentence direct comparison of the two stocks (risk/reward, investor profile fit)
- portfolioRecommendation: concrete suggestion on how a retail investor might weight these together (or not)
- disclaimer: standard financial disclaimer`,
  { schema: SYNTHESIS_SCHEMA, phase: 'Synthesize' }
)

log(synthesis.executiveSummary)

export default { decomposition, findings: validFindings, synthesis }
