function trimTrailingSeparators(path: string, rootLength: number): string {
  let end = path.length
  while (end > rootLength && path[end - 1] === '/') {
    end -= 1
  }
  return path.slice(0, end)
}

export function isWindowsPath(path: string): boolean {
  const trimmed = path.trim()
  return /^[a-zA-Z]:(?:[\\/]|$)/.test(trimmed)
    || /^[\\/]{2}[^\\/]/.test(trimmed)
    || trimmed.includes('\\')
}

export function normalizePathForComparison(path: string): string {
  const trimmed = path.trim()
  if (!trimmed) return ''

  if (isWindowsPath(trimmed)) {
    const normalized = trimmed.replace(/[\\/]+/g, '/')
    const rootLength = /^[a-zA-Z]:\/$/.test(normalized)
      ? normalized.length
      : normalized.startsWith('//')
        ? 2
        : 0
    return trimTrailingSeparators(normalized, rootLength).toLowerCase()
  }

  const normalized = trimmed.replace(/\/{2,}/g, '/')
  return trimTrailingSeparators(normalized, normalized === '/' ? 1 : 0)
}

export function isSameOrDescendantPath(path: string, possibleParent: string): boolean {
  if (path === possibleParent) return true
  if (!possibleParent) return false
  return path.startsWith(possibleParent.endsWith('/') ? possibleParent : `${possibleParent}/`)
}
