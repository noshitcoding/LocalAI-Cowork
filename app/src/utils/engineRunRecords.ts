export type EngineRunEventRow = {
  id: string
  runId: string
  sequence: number
  eventType: string
  summary: string
  payloadJson: string | null
  redactionLevel: string
  createdAt: string
}

export type EngineRunArtifactRow = {
  id: string
  runId: string
  kind: string
  path: string
  title: string | null
  summary: string | null
  createdAt: string
}

type RawRecord = Record<string, unknown>

const ISO_EPOCH = new Date(0).toISOString()

const asRecord = (value: unknown): RawRecord =>
  value && typeof value === 'object' ? value as RawRecord : {}

const asString = (value: unknown, fallback = ''): string =>
  typeof value === 'string' ? value : fallback

const asNullableString = (value: unknown): string | null =>
  typeof value === 'string' ? value : null

const asNumber = (value: unknown, fallback = 0): number => {
  if (typeof value === 'number' && Number.isFinite(value)) return value
  if (typeof value === 'string' && value.trim()) {
    const parsed = Number(value)
    if (Number.isFinite(parsed)) return parsed
  }
  return fallback
}

const asTimestampString = (primary: unknown, secondary?: unknown): string => {
  const value = asString(primary) || asString(secondary)
  return value || ISO_EPOCH
}

export function normalizeEngineRunEvent(value: unknown): EngineRunEventRow | null {
  const event = asRecord(value)
  const id = asString(event.id)
  if (!id) return null

  return {
    id,
    runId: asString(event.runId ?? event.run_id),
    sequence: asNumber(event.sequence),
    eventType: asString(event.eventType ?? event.event_type, 'event'),
    summary: asString(event.summary, 'Event'),
    payloadJson: asNullableString(event.payloadJson ?? event.payload_json),
    redactionLevel: asString(event.redactionLevel ?? event.redaction_level, 'none'),
    createdAt: asTimestampString(event.createdAt ?? event.created_at),
  }
}

export function normalizeEngineRunArtifact(value: unknown): EngineRunArtifactRow | null {
  const artifact = asRecord(value)
  const id = asString(artifact.id)
  if (!id) return null

  return {
    id,
    runId: asString(artifact.runId ?? artifact.run_id),
    kind: asString(artifact.kind, 'artifact'),
    path: asString(artifact.path),
    title: asNullableString(artifact.title),
    summary: asNullableString(artifact.summary),
    createdAt: asTimestampString(artifact.createdAt ?? artifact.created_at),
  }
}
