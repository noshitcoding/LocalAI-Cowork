import { describe, expect, it } from 'vitest'
import { currencyInput, euroInput, formatCurrencyMicros, formatEuroMicros, parseCurrencyMicros, parseEuroMicros } from './money'

describe('exact wallet amounts', () => {
  it('parses decimal amounts without floating-point multiplication', () => {
    expect(parseEuroMicros('0.123456')).toBe(123456)
    expect(parseEuroMicros('12,000001')).toBe(12000001)
    expect(parseEuroMicros('1')).toBe(1000000)
    expect(parseEuroMicros('0')).toBe(0)
    expect(parseEuroMicros('9007199254.740991')).toBe(Number.MAX_SAFE_INTEGER)
  })
  it.each(['1e3', '-1', '1.0000001', '1,234.56', 'NaN', 'Infinity', '9007199254.740992', '', '.50'])('rejects ambiguous or inexact input: %s', value => {
    expect(parseEuroMicros(value)).toBeNull()
  })
  it('retains all micro-units in editing and display', () => {
    expect(euroInput(123456)).toBe('0.123456')
    expect(euroInput(12000000)).toBe('12')
    expect(formatEuroMicros(123456, 'de')).toBe('0,123456 €')
    expect(formatEuroMicros(12000000, 'en')).toBe('12.00 €')
    expect(formatEuroMicros(-1, 'de')).toBe('−0,000001 €')
  })
  it('labels the original currency without changing its micro-units', () => {
    expect(formatCurrencyMicros(123456, 'EUR', 'de')).toBe('0,123456 €')
    expect(formatCurrencyMicros(123456, 'USD', 'de')).toBe('0,123456 USD')
    expect(formatCurrencyMicros(123456, 'USD', 'en')).toBe('0.123456 USD')
    expect(formatCurrencyMicros(12000000, 'JPY', 'en')).toBe('12.00 JPY')
    expect(parseCurrencyMicros(currencyInput(123456))).toBe(123456)
  })
})
