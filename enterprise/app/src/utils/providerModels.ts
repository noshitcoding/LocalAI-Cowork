function modelSuffix(model: string): string {
  const trimmed = model.trim()
  return trimmed.split('/').filter(Boolean).at(-1) ?? trimmed
}

export function normalizeProviderModels(models: string[]): string[] {
  const seen = new Set<string>()
  return models
    .map((model) => model.trim())
    .filter((model) => {
      const key = model.toLowerCase()
      if (!model || seen.has(key)) return false
      seen.add(key)
      return true
    })
}

export function resolveProviderModelFromCatalog(configuredModel: string, models: string[]): string {
  const configured = configuredModel.trim()
  const normalizedModels = normalizeProviderModels(models)
  if (normalizedModels.length === 0) return configured
  if (!configured) return normalizedModels.length === 1 ? normalizedModels[0] : ''

  const lowerConfigured = configured.toLowerCase()
  const exact = normalizedModels.find((model) => model.toLowerCase() === lowerConfigured)
  if (exact) return exact

  const configuredSuffix = modelSuffix(configured).toLowerCase()
  const suffixMatch = normalizedModels.find((model) => modelSuffix(model).toLowerCase() === configuredSuffix)
  if (suffixMatch) return suffixMatch

  const stemMatches = normalizedModels.filter((model) => {
    const candidateSuffix = modelSuffix(model).toLowerCase()
    return candidateSuffix.startsWith(`${configuredSuffix}-`)
      || configuredSuffix.startsWith(`${candidateSuffix}-`)
  })
  if (stemMatches.length === 1) return stemMatches[0]

  return normalizedModels.length === 1 ? normalizedModels[0] : configured
}
