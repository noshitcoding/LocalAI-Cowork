import { z } from 'zod'

export const authTokensSchema = z.object({
  schema_version: z.number().int(),
  access_token: z.string().min(32),
  access_expires_at: z.string().datetime({ offset: true }),
  // Browser sessions keep this in a host-only HttpOnly cookie.
  refresh_token: z.string().min(32).optional(),
  refresh_expires_at: z.string().datetime({ offset: true }),
  user_id: z.string().uuid(),
  session_id: z.string().uuid(),
})
export type AuthTokens = z.infer<typeof authTokensSchema>
