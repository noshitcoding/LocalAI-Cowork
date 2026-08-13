import { openUrl } from '@tauri-apps/plugin-opener'
import { createElement, Fragment, useMemo } from 'react'
import type { MouseEvent as ReactMouseEvent, ReactNode } from 'react'
import snarkdown from 'snarkdown'
import { extractWebSearchSources } from '../utils/webSearchSources'
import { tr } from '../i18n'
import './MarkdownChatText.css'

type MarkdownChatTextProps = {
  content: string
}

function openExternalUrl(url: string) {
  void openUrl(url).catch(() => {
    if (typeof window !== 'undefined') {
      window.open(url, '_blank', 'noopener,noreferrer')
    }
  })
}

function safeExternalUrl(value: string): string | null {
  try {
    const url = new URL(value)
    return ['http:', 'https:', 'mailto:'].includes(url.protocol) ? url.href : null
  } catch {
    return null
  }
}

function splitTableRow(line: string): string[] {
  const normalized = line.trim().replace(/^\|/, '').replace(/\|$/, '')
  const cells: string[] = []
  let current = ''
  let escaped = false

  for (const character of normalized) {
    if (escaped) {
      current += character
      escaped = false
    } else if (character === '\\') {
      escaped = true
    } else if (character === '|') {
      cells.push(current.trim())
      current = ''
    } else {
      current += character
    }
  }

  cells.push(current.trim())
  return cells
}

function expandGfmTables(markdown: string): string {
  const lines = markdown.split(/\r?\n/)
  const result: string[] = []
  let inFence = false

  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index] ?? ''
    if (line.trimStart().startsWith('```')) {
      inFence = !inFence
      result.push(line)
      continue
    }

    const headers = splitTableRow(line)
    const separators = splitTableRow(lines[index + 1] ?? '')
    const isTable = !inFence
      && headers.length > 1
      && headers.length === separators.length
      && separators.every((cell) => /^:?-+:?$/.test(cell))

    if (!isTable) {
      result.push(line)
      continue
    }

    const rows: string[][] = []
    index += 2
    while (index < lines.length) {
      const rowLine = lines[index] ?? ''
      const cells = splitTableRow(rowLine)
      if (!rowLine.includes('|') || cells.length !== headers.length) break
      rows.push(cells)
      index += 1
    }
    index -= 1

    const headerHtml = headers.map((cell) => `<th>${snarkdown(cell)}</th>`).join('')
    const bodyHtml = rows
      .map((row) => `<tr>${row.map((cell) => `<td>${snarkdown(cell)}</td>`).join('')}</tr>`)
      .join('')
    result.push(`<table><thead><tr>${headerHtml}</tr></thead><tbody>${bodyHtml}</tbody></table>`)
  }

  return result.join('\n')
}

const ALLOWED_MARKDOWN_TAGS = new Set([
  'a', 'blockquote', 'br', 'code', 'del', 'em', 'h1', 'h2', 'h3',
  'h4', 'h5', 'h6', 'hr', 'li', 'ol', 'p', 'pre', 's', 'strong',
  'table', 'tbody', 'td', 'th', 'thead', 'tr', 'ul',
])

function sanitizedReactNode(node: Node, key: string): ReactNode {
  if (node.nodeType === 3) return node.textContent
  if (node.nodeType !== 1) return null

  const element = node as HTMLElement
  const tagName = element.tagName.toLowerCase()
  const children = Array.from(element.childNodes)
    .map((child, index) => sanitizedReactNode(child, `${key}-${index}`))

  if (tagName === 'img') {
    const alt = element.getAttribute('alt')?.trim()
    return (
      <span className="chat-markdown-image-placeholder" key={key}>
        {alt ? `[${tr('Image')}: ${alt}]` : `[${tr('Image')}]`}
      </span>
    )
  }

  if (!ALLOWED_MARKDOWN_TAGS.has(tagName)) {
    return createElement(Fragment, { key }, children)
  }

  if (tagName === 'a') {
    const safeHref = safeExternalUrl(element.getAttribute('href') ?? '')
    if (!safeHref) return createElement('span', { key }, children)
    return createElement('a', {
      key,
      href: safeHref,
      rel: 'noreferrer',
      onClick: (event: ReactMouseEvent<HTMLAnchorElement>) => {
        event.preventDefault()
        openExternalUrl(safeHref)
      },
    }, children)
  }

  const props: Record<string, unknown> = { key }
  if (tagName === 'ol') {
    const start = Number.parseInt(element.getAttribute('start') ?? '', 10)
    if (Number.isFinite(start)) props.start = start
  }
  if (tagName === 'code') {
    const languageClass = element.className.match(/(?:^|\s)(language-[A-Za-z0-9_-]+)/)?.[1]
    if (languageClass) props.className = languageClass
  }

  if (tagName === 'br' || tagName === 'hr') return createElement(tagName, props)
  return createElement(tagName, props, children)
}

function renderSafeMarkdown(markdown: string): ReactNode[] {
  const document = new DOMParser().parseFromString(
    `<body>${snarkdown(expandGfmTables(markdown))}</body>`,
    'text/html',
  )
  return Array.from(document.body.childNodes)
    .map((node, index) => sanitizedReactNode(node, `markdown-${index}`))
}

export function MarkdownChatText({ content }: MarkdownChatTextProps) {
  const extracted = extractWebSearchSources(content)
  const renderedMarkdown = useMemo(
    () => renderSafeMarkdown(extracted.content),
    [extracted.content],
  )

  return (
    <>
      <div className="chat-markdown">
        {renderedMarkdown}
      </div>
      {extracted.sources.length > 0 && (
        <div className="message-sources">
          {extracted.sources.map((source, index) => (
            <button
              type="button"
              key={`${source.url}-${index}`}
              className="message-source-chip"
              title={source.url}
              aria-label={`${tr('Open source')}: ${source.title}`}
              onClick={() => openExternalUrl(source.url)}
            >
              {source.title}
            </button>
          ))}
        </div>
      )}
    </>
  )
}
