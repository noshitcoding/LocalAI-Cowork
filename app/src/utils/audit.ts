/**
 * Audit event logger.
 * Safely writes audit events to the backend when available.
 * Uses safeInvokeVoid so it never breaks product flows.
 */

import { safeInvokeVoid } from './safeInvoke'
import { redactRecord } from '../security/redaction'

export async function writeAuditEvent(
  area: string,
  action: string,
  details?: Record<string, unknown>
): Promise<void> {
  await safeInvokeVoid('audit_event', {
    area,
    action,
    details: details ? redactRecord(details) : null,
  })
}
