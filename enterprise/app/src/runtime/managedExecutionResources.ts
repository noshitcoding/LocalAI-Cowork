import {z} from 'zod'
export const managedExecutionResourceSchema=z.object({id:z.uuid(),revision:z.number().int().positive(),name:z.string(),kind:z.enum(['skill','agent_role','mcp']),available:z.boolean(),required_capabilities:z.array(z.string())})
export type ManagedExecutionResource=z.infer<typeof managedExecutionResourceSchema>
