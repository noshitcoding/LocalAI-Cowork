import { createContext } from 'react'
import type { RemoteRuntimeClient } from '../runtime/runtimeClient'

export type ManagementSecurity = {
  client: RemoteRuntimeClient
  requiredMfa: boolean
  providerVerified?: boolean
  providerExpiresAt?: string
}

export const ManagementSecurityContext = createContext<ManagementSecurity | null>(null)
