// Live acceptance uses operator-supplied, one-use factors. It never reads a TOTP seed.
let codes
export function nextManagementSecondFactor() {
  if (!codes) {
    if (process.env.COWORK_TEST_ADMIN_SECOND_FACTORS) {
      try { codes = JSON.parse(process.env.COWORK_TEST_ADMIN_SECOND_FACTORS) }
      catch { throw new Error('COWORK_TEST_ADMIN_SECOND_FACTORS must be a JSON array of fresh one-use codes') }
    } else codes = process.env.COWORK_TEST_ADMIN_SECOND_FACTOR ? [process.env.COWORK_TEST_ADMIN_SECOND_FACTOR] : []
    if (!Array.isArray(codes) || codes.some(code => typeof code !== 'string' || !code.trim() || code.length > 128)) {
      throw new Error('Second-factor input must contain only nonempty codes of at most 128 characters')
    }
  }
  const code = codes.shift()
  if (!code) throw new Error('Enroll management MFA first, then supply a fresh COWORK_TEST_ADMIN_SECOND_FACTOR, or COWORK_TEST_ADMIN_SECOND_FACTORS JSON array for each sign-in. Codes are not reused.')
  return code.trim()
}
