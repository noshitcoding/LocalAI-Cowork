import { limitRollingLines } from '../utils/messageDisplay'
import { tr } from '../i18n'

type MessageThinkingProps = {
  content?: string
  limitToRollingWindow: boolean
  streaming?: boolean
}

type MessageStreamPanelProps = MessageThinkingProps & {
  title: string
  className?: string
}

function MessageStreamPanel({ content, limitToRollingWindow, title, className, streaming }: MessageStreamPanelProps) {
  const hasContent = Boolean(content?.trim())
  if (!hasContent && !streaming) return null

  const visibleContent = hasContent
    ? (limitToRollingWindow ? limitRollingLines(content!, 50) : content!)
    : ''

  return (
    <details className={`thinking-panel ${className ?? ''}`.trim()} open={streaming || hasContent}>
      <summary className="thinking-header">
        <span>{title}</span>
        {limitToRollingWindow && <span className="thinking-limit">{tr("latest 50 lines")}</span>}
      </summary>
      <pre aria-live={streaming ? 'polite' : undefined}>{visibleContent}</pre>
    </details>
  )
}

export function MessageThinking({ content, limitToRollingWindow, streaming }: MessageThinkingProps) {
  return (
    <MessageStreamPanel
      content={content}
      limitToRollingWindow={limitToRollingWindow}
      streaming={streaming}
      title={tr("Thinking")}
    />
  )
}

export function MessageVerbose({ content, limitToRollingWindow }: MessageThinkingProps) {
  return (
    <MessageStreamPanel
      content={content}
      limitToRollingWindow={limitToRollingWindow}
      title={tr("Verbose Live")}
      className="verbose-panel"
    />
  )
}
