#!/usr/bin/env node

import readline from 'node:readline'

const PROTOCOL_VERSION = '2024-11-05'
const SERVER_INFO = {
  name: 'localai-cowork-duckduckgo-websearch',
  version: '1.0.0',
}

const DEFAULTS = {
  maxResults: toPositiveInt(process.env.DDG_MAX_RESULTS, 5),
  region: (process.env.DDG_REGION || 'wt-wt').trim() || 'wt-wt',
  safeSearch: normalizeSafeSearch(process.env.DDG_SAFESEARCH || 'moderate'),
  timeoutMs: toPositiveInt(process.env.DDG_TIMEOUT_MS, 10000),
  endpoint: (process.env.DDG_HTML_ENDPOINT || 'https://html.duckduckgo.com/html/').trim(),
}

function toPositiveInt(value, fallback) {
  const parsed = Number.parseInt(String(value), 10)
  return Number.isFinite(parsed) && parsed > 0 ? parsed : fallback
}

function normalizeSafeSearch(value) {
  const safe = String(value || 'moderate').toLowerCase().trim()
  if (safe === 'off' || safe === 'strict' || safe === 'moderate') {
    return safe
  }
  return 'moderate'
}

function safeSearchToKp(value) {
  if (value === 'off') return '-2'
  if (value === 'strict') return '1'
  return '-1'
}

function decodeHtmlEntities(input) {
  return input
    .replace(/&amp;/g, '&')
    .replace(/&quot;/g, '"')
    .replace(/&#39;/g, "'")
    .replace(/&lt;/g, '<')
    .replace(/&gt;/g, '>')
    .replace(/&#x2F;/g, '/')
    .replace(/&#x27;/g, "'")
    .replace(/&#96;/g, '`')
    .replace(/\s+/g, ' ')
    .trim()
}

function extractTargetUrl(rawHref) {
  try {
    const absolute = new URL(rawHref, 'https://duckduckgo.com')
    const wrapped = absolute.searchParams.get('uddg')
    if (wrapped) {
      return decodeURIComponent(wrapped)
    }
    return absolute.toString()
  } catch {
    return rawHref
  }
}

function parseResultsFromHtml(html, maxResults) {
  const anchorRegex = /<a[^>]*class=\"result__a\"[^>]*href=\"([^\"]+)\"[^>]*>([\s\S]*?)<\/a>/gi
  const snippetRegex = /<a[^>]*class=\"result__a\"[^>]*>[\s\S]*?<\/a>[\s\S]{0,1200}?<a[^>]*class=\"result__snippet\"[^>]*>([\s\S]*?)<\/a>/i

  const results = []
  let match
  while ((match = anchorRegex.exec(html)) !== null && results.length < maxResults) {
    const href = match[1]
    const title = decodeHtmlEntities(match[2].replace(/<[^>]+>/g, ''))
    const blockStart = match.index
    const blockEnd = Math.min(blockStart + 2000, html.length)
    const block = html.slice(blockStart, blockEnd)
    const snippetMatch = block.match(snippetRegex)
    const snippet = snippetMatch
      ? decodeHtmlEntities(snippetMatch[1].replace(/<[^>]+>/g, ''))
      : ''

    if (!title) {
      continue
    }

    results.push({
      title,
      url: extractTargetUrl(href),
      snippet,
    })
  }

  return results
}

function formatTextResult(query, options, results) {
  const header = `DuckDuckGo search for: ${query}`
  const meta = `region=${options.region}, safesearch=${options.safeSearch}, maxResults=${options.maxResults}`
  if (results.length === 0) {
    return `${header}\n${meta}\n\nNo results found.`
  }

  const lines = results.map((item, index) => {
    const snippetLine = item.snippet ? `\n   ${item.snippet}` : ''
    return `${index + 1}. ${item.title}\n   ${item.url}${snippetLine}`
  })
  return `${header}\n${meta}\n\n${lines.join('\n\n')}`
}

function asError(id, code, message, data) {
  const payload = {
    jsonrpc: '2.0',
    id: id ?? null,
    error: {
      code,
      message,
    },
  }
  if (typeof data !== 'undefined') {
    payload.error.data = data
  }
  return payload
}

function send(payload) {
  process.stdout.write(`${JSON.stringify(payload)}\n`)
}

async function searchDuckDuckGo(args) {
  const query = String(args?.query || '').trim()
  if (!query) {
    throw new Error('Missing required argument: query')
  }

  const maxResults = toPositiveInt(args?.maxResults, DEFAULTS.maxResults)
  const region = String(args?.region || DEFAULTS.region).trim() || DEFAULTS.region
  const safeSearch = normalizeSafeSearch(args?.safeSearch || DEFAULTS.safeSearch)
  const timeoutMs = toPositiveInt(args?.timeoutMs, DEFAULTS.timeoutMs)

  const params = new URLSearchParams({
    q: query,
    kl: region,
    kp: safeSearchToKp(safeSearch),
  })

  const controller = new AbortController()
  const timeout = setTimeout(() => controller.abort(), timeoutMs)

  let response
  try {
    response = await fetch(`${DEFAULTS.endpoint}?${params.toString()}`, {
      method: 'GET',
      signal: controller.signal,
      headers: {
        'user-agent': 'LocalAI Cowork DuckDuckGo MCP/1.0',
      },
    })
  } finally {
    clearTimeout(timeout)
  }

  if (!response.ok) {
    throw new Error(`DuckDuckGo request failed with status ${response.status}`)
  }

  const html = await response.text()
  const results = parseResultsFromHtml(html, maxResults)
  const text = formatTextResult(query, { region, safeSearch, maxResults }, results)

  return {
    text,
    structuredContent: {
      query,
      region,
      safeSearch,
      maxResults,
      count: results.length,
      results,
    },
  }
}

const rl = readline.createInterface({
  input: process.stdin,
  crlfDelay: Infinity,
})

rl.on('line', async (line) => {
  const trimmed = line.trim()
  if (!trimmed) {
    return
  }

  let request
  try {
    request = JSON.parse(trimmed)
  } catch {
    send(asError(null, -32700, 'Parse error'))
    return
  }

  const id = Object.prototype.hasOwnProperty.call(request, 'id') ? request.id : null
  const method = request?.method

  if (method === 'initialize') {
    send({
      jsonrpc: '2.0',
      id,
      result: {
        protocolVersion: PROTOCOL_VERSION,
        serverInfo: SERVER_INFO,
        capabilities: {
          tools: {},
        },
      },
    })
    return
  }

  if (method === 'tools/list') {
    send({
      jsonrpc: '2.0',
      id,
      result: {
        tools: [
          {
            name: 'search_web',
            description: 'Search the web with DuckDuckGo and return top results.',
            inputSchema: {
              type: 'object',
              properties: {
                query: {
                  type: 'string',
                  description: 'Search query string.',
                },
                maxResults: {
                  type: 'integer',
                  minimum: 1,
                  maximum: 20,
                  description: 'Maximum number of results to return (default from DDG_MAX_RESULTS).',
                },
                region: {
                  type: 'string',
                  description: 'DuckDuckGo region code like wt-wt, us-en, de-de.',
                },
                safeSearch: {
                  type: 'string',
                  enum: ['off', 'moderate', 'strict'],
                  description: 'Safe search level.',
                },
                timeoutMs: {
                  type: 'integer',
                  minimum: 1000,
                  maximum: 60000,
                  description: 'Request timeout in milliseconds.',
                },
              },
              required: ['query'],
            },
          },
        ],
      },
    })
    return
  }

  if (method === 'tools/call') {
    const toolName = request?.params?.name
    if (toolName !== 'search_web') {
      send(asError(id, -32601, `Unknown tool: ${String(toolName || '')}`))
      return
    }

    try {
      const output = await searchDuckDuckGo(request?.params?.arguments || {})
      send({
        jsonrpc: '2.0',
        id,
        result: {
          content: [
            {
              type: 'text',
              text: output.text,
            },
          ],
          structuredContent: output.structuredContent,
        },
      })
    } catch (error) {
      send(asError(id, -32000, error instanceof Error ? error.message : String(error)))
    }
    return
  }

  if (id !== null) {
    send(asError(id, -32601, `Method not found: ${String(method || '')}`))
  }
})

rl.on('close', () => {
  process.exit(0)
})
