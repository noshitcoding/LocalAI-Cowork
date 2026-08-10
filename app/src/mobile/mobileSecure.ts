import { invoke } from '@tauri-apps/api/core'

export const IS_ANDROID_SHELL = import.meta.env.VITE_COWORK_ANDROID === 'true'

type SecretResponse = { value: string | null }
const browserFallback = new Map<string, string>()

function fallbackKey(namespace: string, key: string): string {
  return `${namespace}\0${key}`
}

export async function mobileSecureSet(namespace: string, key: string, value: string): Promise<void> {
  if (!IS_ANDROID_SHELL) {
    browserFallback.set(fallbackKey(namespace, key), value)
    return
  }
  await invoke('plugin:mobile-secure|store', { namespace, key, value })
}

export async function mobileSecureGet(namespace: string, key: string): Promise<string | null> {
  if (!IS_ANDROID_SHELL) return browserFallback.get(fallbackKey(namespace, key)) ?? null
  const response = await invoke<SecretResponse>('plugin:mobile-secure|retrieve', { namespace, key })
  return response.value
}

export async function mobileSecureDelete(namespace: string, key: string): Promise<void> {
  if (!IS_ANDROID_SHELL) {
    browserFallback.delete(fallbackKey(namespace, key))
    return
  }
  await invoke('plugin:mobile-secure|remove', { namespace, key })
}

export async function unlockWithBiometrics(): Promise<'unlocked' | 'unavailable'> {
  if (!IS_ANDROID_SHELL) return 'unavailable'
  const { authenticate, checkStatus } = await import('@tauri-apps/plugin-biometric')
  const status = await checkStatus()
  if (!status.isAvailable) return 'unavailable'
  await authenticate('Unlock Open Cowork', {
    allowDeviceCredential: true,
    title: 'Unlock Open Cowork',
    subtitle: 'Confirm your identity to access cached runs and server sessions',
    confirmationRequired: true,
  })
  return 'unlocked'
}

const PIN_NAMESPACE = 'app_lock'
const PIN_KEY = 'pin_verifier_v1'

type PinVerifier = { salt: string; digest: string; iterations: number }

export async function hasMobilePin(): Promise<boolean> {
  return (await mobileSecureGet(PIN_NAMESPACE, PIN_KEY)) !== null
}

export async function setMobilePin(pin: string): Promise<void> {
  validatePin(pin)
  const salt = crypto.getRandomValues(new Uint8Array(16))
  const iterations = 250_000
  const digest = await derivePin(pin, salt, iterations)
  await mobileSecureSet(PIN_NAMESPACE, PIN_KEY, JSON.stringify({
    salt: toBase64(salt),
    digest: toBase64(digest),
    iterations,
  } satisfies PinVerifier))
}

export async function verifyMobilePin(pin: string): Promise<boolean> {
  const encoded = await mobileSecureGet(PIN_NAMESPACE, PIN_KEY)
  if (!encoded) return false
  const verifier = JSON.parse(encoded) as PinVerifier
  const actual = await derivePin(pin, fromBase64(verifier.salt), verifier.iterations)
  const expected = fromBase64(verifier.digest)
  if (actual.length !== expected.length) return false
  let difference = 0
  for (let index = 0; index < actual.length; index += 1) {
    difference |= (actual[index] ?? 0) ^ (expected[index] ?? 0)
  }
  return difference === 0
}

function validatePin(pin: string): void {
  if (!/^\d{6,12}$/.test(pin)) throw new Error('PIN must contain 6 to 12 digits')
}

async function derivePin(pin: string, salt: Uint8Array, iterations: number): Promise<Uint8Array> {
  const material = await crypto.subtle.importKey(
    'raw',
    new TextEncoder().encode(pin),
    'PBKDF2',
    false,
    ['deriveBits'],
  )
  const bits = await crypto.subtle.deriveBits(
    { name: 'PBKDF2', hash: 'SHA-256', salt: salt as BufferSource, iterations },
    material,
    256,
  )
  return new Uint8Array(bits)
}

export function toBase64(bytes: Uint8Array): string {
  let binary = ''
  for (const byte of bytes) binary += String.fromCharCode(byte)
  return btoa(binary)
}

export function fromBase64(value: string): Uint8Array {
  const binary = atob(value)
  return Uint8Array.from(binary, (character) => character.charCodeAt(0))
}

export function resetMobileSecureForTests(): void {
  if (IS_ANDROID_SHELL) throw new Error('Cannot reset Android secure storage from JavaScript')
  browserFallback.clear()
}
