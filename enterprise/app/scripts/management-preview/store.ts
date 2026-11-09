import { userId } from './fixtures'
const readOnly = async () => { throw new Error('TESTDATEN: Änderungen und Anmeldungen sind in dieser Designvorschau deaktiviert.') }
const account = { serverUrl: 'https://verwaltung.example.invalid', userId, email: 'alex@example.invalid', status: 'authenticated', error: null, logout: async () => {}, loginFleet: readOnly, loginPasskey: readOnly, loginOidc: readOnly, linkOidc: readOnly }
export const useRemoteRuntimeStore = Object.assign(<T = typeof account>(selector?: (value: typeof account) => T): T => selector ? selector(account) : account as T, { getState: () => account, subscribe: () => () => {} })
export const remoteDeviceId = () => 'preview-device'
export const remoteTokenProvider = () => async () => null
export const remoteRuntimeClient = () => ({
  authenticationAssurance: async () => ({ required: true, session_verified: true, totp_enabled: true, passkey_registered: false, enrollment_allowed: false, reauthentication_required: false, provider_verified: false }),
  verifySecondFactor: readOnly, setupTotp: readOnly, enableTotp: readOnly,
  regenerateRecoveryCodes: readOnly, disableTotp: readOnly, registerPasskey: readOnly, removePasskey: readOnly, revokeAuthSession: readOnly,
  passkeysAvailableInContext: () => false, totpStatus: async () => ({ schema_version: 2, enabled: true, unused_recovery_codes: 8 }), listPasskeys: async () => [], listAuthSessions: async () => [],
})
