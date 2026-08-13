import type { ReactNode } from 'react'
import { openUrl } from '@tauri-apps/plugin-opener'
import { extractWebSearchSources } from '../utils/webSearchSources'
import { tr } from '../i18n'

type HighlightedChatTextProps = {
  content: string
}

export function HighlightedChatText({ content }: HighlightedChatTextProps) {
  const extracted = extractWebSearchSources(content)
  const parts: ReactNode[] = []
  let lastIndex = 0
  let match: RegExpExecArray | null
  const commandTokenPattern = /(^|[\s([{])\/[A-Za-z][A-Za-z0-9_-]*/g

  while ((match = commandTokenPattern.exec(extracted.content)) !== null) {
    const prefix = match[1] ?? ''
    const commandStart = match.index + prefix.length
    const commandEnd = match.index + match[0].length

    if (lastIndex < commandStart) {
      parts.push(extracted.content.slice(lastIndex, commandStart))
    }

    parts.push(
      <span className="chat-command-highlight" key={`${commandStart}-${commandEnd}`}>
        {extracted.content.slice(commandStart, commandEnd)}
      </span>,
    )

    lastIndex = commandEnd
  }

  if (lastIndex < extracted.content.length) {
    parts.push(extracted.content.slice(lastIndex))
  }

  const handleSourceClick = (url: string) => {
    void openUrl(url).catch(() => {
      if (typeof window !== 'undefined') {
        window.open(url, '_blank', 'noopener,noreferrer')
      }
    })
  }

  return (
    <>
      {parts.length > 0 ? parts : extracted.content}
      {extracted.sources.length > 0 && (
        <div className="message-sources">
          {extracted.sources.map((source, index) => (
            <button
              type="button"
              key={`${source.url}-${index}`}
              className="message-source-chip"
              title={source.url}
              aria-label={`${tr("Open source")}: ${source.title}`}
              onClick={() => handleSourceClick(source.url)}
            >
              {source.title}
            </button>
          ))}
        </div>
      )}
    </>
  )
}
