/** Decimal currency input is converted with integer arithmetic, without FX. */
export function parseCurrencyMicros(input: string): number | null {
  const match = /^(\d{1,16})(?:[.,](\d{1,6}))?$/.exec(input.trim())
  if (!match) return null
  const micros = BigInt(match[1]) * 1_000_000n + BigInt((match[2] ?? '').padEnd(6, '0'))
  return micros <= BigInt(Number.MAX_SAFE_INTEGER) ? Number(micros) : null
}

export function currencyInput(micros: number): string {
  if (!Number.isSafeInteger(micros) || micros < 0) return ''
  const value = BigInt(micros), fraction = (value % 1_000_000n).toString().padStart(6, '0').replace(/0+$/, '')
  return `${value / 1_000_000n}${fraction ? `.${fraction}` : ''}`
}

export function formatCurrencyMicros(micros: number, currency: string, locale: string): string {
  if (!Number.isSafeInteger(micros) || !/^[A-Z]{3}$/.test(currency)) return '—'
  const value = BigInt(micros), absolute = value < 0n ? -value : value
  const separator = new Intl.NumberFormat(locale).formatToParts(1.1).find(part => part.type === 'decimal')?.value ?? '.'
  const fraction = (absolute % 1_000_000n).toString().padStart(6, '0').replace(/0+$/, '').padEnd(2, '0')
  return `${value < 0n ? '−' : ''}${(absolute / 1_000_000n).toLocaleString(locale)}${separator}${fraction} ${currency === 'EUR' ? '€' : currency}`
}

export const parseEuroMicros = parseCurrencyMicros
export const euroInput = currencyInput
export const formatEuroMicros = (micros: number, locale: string) => formatCurrencyMicros(micros, 'EUR', locale)
