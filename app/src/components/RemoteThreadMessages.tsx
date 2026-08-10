import { MessageSquareText } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'

import type { MessageRecord } from '../runtime/contracts'
import type { RemoteRuntimeClient } from '../runtime/runtimeClient'
import './RemoteThreadMessages.css'

type RemoteThreadMessagesProps = {
  client: RemoteRuntimeClient | null
  threadId: string
  reloadKey?: number
  initialMessages?: MessageRecord[]
  onLoaded?: (messages: MessageRecord[]) => void
}

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

function contentText(content: unknown): string {
  if (typeof content === 'string') return content
  if (content && typeof content === 'object') {
    const record = content as Record<string, unknown>
    for (const key of ['text', 'response', 'output', 'message']) {
      if (typeof record[key] === 'string') return record[key]
    }
  }
  return JSON.stringify(content, null, 2) ?? String(content)
}

export default function RemoteThreadMessages({
  client,
  threadId,
  reloadKey = 0,
  initialMessages,
  onLoaded,
}: RemoteThreadMessagesProps) {
  const [messages, setMessages] = useState(initialMessages ?? [])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const onLoadedRef = useRef(onLoaded)

  useEffect(() => { onLoadedRef.current = onLoaded }, [onLoaded])

  useEffect(() => {
    setMessages(initialMessages ?? [])
  }, [initialMessages, threadId])

  useEffect(() => {
    if (!client) return
    let canceled = false
    setLoading(true)
    setError(null)
    void client.listThreadMessages(threadId, 1_000)
      .then((loaded) => {
        if (canceled) return
        setMessages(loaded)
        onLoadedRef.current?.(loaded)
      })
      .catch((cause) => { if (!canceled) setError(messageOf(cause)) })
      .finally(() => { if (!canceled) setLoading(false) })
    return () => { canceled = true }
  }, [client, reloadKey, threadId])

  return (
    <section className="remote-message-panel">
      <header className="remote-message-header">
        <div><MessageSquareText size={15} /><span><h2>Conversation</h2><p>Durable messages linked to this thread.</p></span></div>
        <span>{messages.length}</span>
      </header>
      {loading && messages.length === 0 ? <p className="remote-message-muted">Loading conversation…</p> : null}
      {!loading && messages.length === 0 ? <p className="remote-message-muted">No durable messages in this legacy run.</p> : null}
      <ol>
        {messages.map((message) => (
          <li key={message.id} className={`role-${message.role}`}>
            <div><strong>{message.role}</strong><time>{new Date(message.created_at).toLocaleString()}</time></div>
            <pre>{contentText(message.content)}</pre>
          </li>
        ))}
      </ol>
      {error ? <div className="remote-inline-error" role="alert">{error}</div> : null}
    </section>
  )
}
