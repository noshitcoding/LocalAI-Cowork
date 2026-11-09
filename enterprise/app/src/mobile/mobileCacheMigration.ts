import type {RemoteRuntimeClient} from '../runtime/runtimeClient'
import type {MobileOutboxOperation} from './mobileOfflineStore'

/** Cached identifiers grant no authority. The user selects an authorized target
 * and receives a fresh operation with no inherited tool/device capability. */
export async function relinkLegacyMobileOperation(client: RemoteRuntimeClient, operation: MobileOutboxOperation, targetId: string): Promise<MobileOutboxOperation> {
  if (!/^[0-9a-f]{8}-[0-9a-f-]{27}$/i.test(targetId)) throw new Error('Select the UUID of the current server target')
  if (operation.kind === 'cancel_run') {
    const target = await client.getRun(targetId)
    if (['completed', 'failed', 'canceled', 'expired'].includes(target.state)) throw new Error('The selected run has already finished')
    return { id: crypto.randomUUID(), kind: 'cancel_run', runId: target.spec.id, createdAt: new Date().toISOString(), attempts: 0 }
  }
  const projects = await client.listProjects()
  if (projects.length > 2000) throw new Error('Select a target in a smaller project list')
  for (const project of projects) {
    const target = (await client.listProjectThreads(project.id)).find(thread => thread.id === targetId)
    if (!target || target.deleted_at) continue
    if (!project.preferred_executor_target) throw new Error('Set the current project executor before requeuing this saved reply')
    const content = operation.request.content, id = crypto.randomUUID()
    return { id, kind: 'thread_message', threadId: target.id, createdAt: new Date().toISOString(), attempts: 0,
      request: { content, run: { thread_id: target.id, project_id: project.id, project_revision: project.revision, project_privacy: project.privacy, task: null, executor_target: project.preferred_executor_target, required_capabilities: [], input: {prompt: content && typeof content === 'object' && 'text' in content ? String(content.text) : ''}, model_profile_id: project.team_id, snapshot_id: null, idempotency_key: id } } }
  }
  throw new Error('The selected thread is unavailable to this account')
}
