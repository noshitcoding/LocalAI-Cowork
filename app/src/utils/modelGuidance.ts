export type ModelGuidance = {
  title: string
  summary: string
  recommendedFor: string
  tradeoff: string
  nameExplanations: ModelNameExplanation[]
}

type GuidancePreset = Omit<ModelGuidance, 'nameExplanations'>
type ModelNameExplanation = {
  text: string
  values?: Record<string, string>
}

const PRESETS = {
  embedding: {
    title: 'Search and retrieval model',
    summary: 'Creates numeric representations of text so related content can be found.',
    recommendedFor: 'Semantic search, document indexing, and knowledge retrieval.',
    tradeoff: 'This model is not intended for writing chat answers.',
  },
  vision: {
    title: 'Image and text model',
    summary: 'Understands visual content together with written instructions.',
    recommendedFor: 'Screenshots, diagrams, scanned pages, and image-based questions.',
    tradeoff: 'Image support also depends on the selected provider and connection.',
  },
  code: {
    title: 'Programming model',
    summary: 'Specializes in understanding and producing software source code.',
    recommendedFor: 'Writing, explaining, reviewing, debugging, and restructuring code.',
    tradeoff: 'Generated changes should still be checked with tests and review.',
  },
  reasoning: {
    title: 'Reasoning-focused model',
    summary: 'Works through difficult questions in several connected steps.',
    recommendedFor: 'Complex planning, analysis, comparisons, and multi-step problems.',
    tradeoff: 'It may respond more slowly or require more computing resources.',
  },
  fast: {
    title: 'Fast everyday model',
    summary: 'Prioritizes short response times for straightforward work.',
    recommendedFor: 'Quick questions, summaries, drafts, translation, and routine chat.',
    tradeoff: 'Use a larger or reasoning-focused model for difficult multi-step work.',
  },
  large: {
    title: 'High-capacity model',
    summary: 'Uses more model capacity for demanding or nuanced work.',
    recommendedFor: 'Long documents, detailed analysis, careful writing, and complex tool workflows.',
    tradeoff: 'It often needs more memory, computing time, or provider cost.',
  },
  general: {
    title: 'General-purpose model',
    summary: 'Balances writing, understanding, and conversation for everyday use.',
    recommendedFor: 'Chat, writing, summarization, translation, and general assistance.',
    tradeoff: 'Choose a specialized model for images, code-heavy tasks, or deep reasoning when available.',
  },
} satisfies Record<string, GuidancePreset>

function hasPattern(value: string, pattern: RegExp): boolean {
  return pattern.test(value)
}

function resolvePreset(model: string): GuidancePreset {
  const normalized = model.trim().toLowerCase()

  if (hasPattern(normalized, /(?:embed|embedding|nomic-embed|text-embedding|bge|(?:^|[-/:])e5(?:$|[-/:])|(?:^|[-/:])gte(?:$|[-/:]))/)) {
    return PRESETS.embedding
  }
  if (hasPattern(normalized, /(?:vision|llava|bakllava|moondream|mllama|pixtral|internvl|minicpm-v|(?:^|[-/:])vl(?:$|[-/:]))/)) {
    return PRESETS.vision
  }
  if (hasPattern(normalized, /(?:code|coder|codellama|devstral|starcoder|codestral|deepseek-coder)/)) {
    return PRESETS.code
  }
  if (hasPattern(normalized, /(?:reason|thinking|deepseek-r1|qwq|(?:^|[-/:])r1(?:$|[-/:])|(?:^|[-/:])o[34](?:$|[-/:])|gpt-5)/)) {
    return PRESETS.reasoning
  }

  const size = normalized.match(/(?:^|[-/:])(\d+(?:\.\d+)?)b(?:$|[-/:])/)
  if (size) {
    const billions = Number(size[1])
    if (billions <= 10) return PRESETS.fast
    if (billions >= 30) return PRESETS.large
  }
  if (hasPattern(normalized, /(?:^|[-/:])(?:nano|mini|small)(?:$|[-/:])/)) return PRESETS.fast
  if (hasPattern(normalized, /(?:^|[-/:])(?:large|max|pro|opus)(?:$|[-/:])/)) return PRESETS.large

  return PRESETS.general
}

function explainModelName(model: string): ModelNameExplanation[] {
  const normalized = model.trim().toLowerCase()
  const explanations: ModelNameExplanation[] = []
  const size = normalized.match(/(?:^|[-/:])(\d+(?:\.\d+)?)b(?:$|[-/:])/)

  if (size) {
    explanations.push({
      text: '“{{size}}B” means approximately {{size}} billion model parameters.',
      values: { size: size[1] },
    })
  }
  if (hasPattern(normalized, /(?:^|[-/:])vl(?:$|[-/:])/)) {
    explanations.push({ text: '“VL” means vision and language: the model is designed to work with images and text.' })
  }
  if (hasPattern(normalized, /(?:^|[-/:])r1(?:$|[-/:])/)) {
    explanations.push({ text: '“R1” is a model-family label commonly used for a reasoning-focused variant.' })
  }
  const quantization = normalized.match(/(?:^|[-/:])(q[2-8](?:_[a-z0-9]+)?)(?:$|[-/:])/)
  if (quantization) {
    explanations.push({
      text: '“{{quantization}}” describes quantization, a compressed number format that reduces memory use.',
      values: { quantization: quantization[1].toUpperCase() },
    })
  }
  const precision = normalized.match(/(?:^|[-/:])((?:bf|f)p(?:8|16|32))(?:$|[-/:])/)
  if (precision) {
    explanations.push({
      text: '“{{precision}}” describes floating-point precision used to store model values.',
      values: { precision: precision[1].toUpperCase() },
    })
  }
  if (normalized.includes('gguf')) {
    explanations.push({ text: '“GGUF” is a file format for running language models locally.' })
  }
  if (normalized.includes('instruct')) {
    explanations.push({ text: '“Instruct” means the model was tuned to follow written instructions.' })
  }
  if (hasPattern(normalized, /(?:^|[-/:])chat(?:$|[-/:])/)) {
    explanations.push({ text: '“Chat” means the model was tuned for dialogue.' })
  }
  if (hasPattern(normalized, /(?:^|[-/:])(?:nano|mini|small)(?:$|[-/:])/)) {
    explanations.push({ text: '“Nano”, “mini”, or “small” identifies a smaller model variant from its provider.' })
  }
  if (normalized.endsWith(':free')) {
    explanations.push({ text: '“Free” is a provider label for a no-charge routing tier; availability and limits depend on the provider.' })
  }

  if (explanations.length === 0) {
    explanations.push({ text: 'This is the provider’s exact technical model identifier; it contains no common abbreviation that can be explained reliably.' })
  }

  return explanations
}

export function getModelGuidance(model: string): ModelGuidance {
  return {
    ...resolvePreset(model),
    nameExplanations: explainModelName(model),
  }
}

export function getModelOptionLabel(model: string): string {
  const guidance = getModelGuidance(model)
  return `${model} — ${guidance.title}`
}
