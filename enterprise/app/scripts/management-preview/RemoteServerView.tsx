import type { ReactNode } from 'react'
import type { RemoteRuntimeClient } from '../../src/runtime/runtimeClient'
import { ManagementSecurityContext } from '../../src/components/managementSecurityContext'
import { remoteRuntimeClient } from './store'
/** Preview-only wrapper: the harness supplies synthetic authenticated state. */
export default function RemoteServerView({ managementContent }: { managementContent?: ReactNode; headingLevel?: number }) { return <ManagementSecurityContext.Provider value={{ client:remoteRuntimeClient() as unknown as RemoteRuntimeClient, requiredMfa:true }}>{managementContent}</ManagementSecurityContext.Provider> }
