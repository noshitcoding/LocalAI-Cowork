#!/usr/bin/env node

import { readFileSync } from 'node:fs'
import { spawnSync } from 'node:child_process'

const HISTORY = process.argv.includes('--history')
const MAX_TEXT_FILE_BYTES = 5 * 1024 * 1024
const PLACEHOLDER = /(?:example|test|dummy|placeholder|redacted|changeme|your[-_]?|must-not|top-secret|url-secret)/i

const secretRules = [
  ['private-key', /-----BEGIN (?:RSA |EC |OPENSSH |DSA )?PRIVATE KEY-----/g],
  ['github-token', /\b(?:gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,})\b/g],
  ['openai-style-key', /\bsk-(?:proj-|svcacct-)?[A-Za-z0-9_-]{20,}\b/g],
  ['google-api-key', /\bAIza[0-9A-Za-z_-]{30,}\b/g],
  ['aws-access-key', /\b(?:AKIA|ASIA)[0-9A-Z]{16}\b/g],
  ['slack-token', /\bxox[baprs]-[A-Za-z0-9-]{20,}\b/g],
  ['stripe-secret', /\b(?:sk|rk)_(?:live|test)_[0-9A-Za-z]{20,}\b/g],
  ['credential-in-url', /\b[a-z][a-z0-9+.-]*:\/\/[^\s/:]+:[^\s/@]+@[^\s]+/gi],
]

const privacyRules = [
  ['private-ipv4', /\b(?:10(?:\.\d{1,3}){3}|192\.168(?:\.\d{1,3}){2}|172\.(?:1[6-9]|2\d|3[01])(?:\.\d{1,3}){2})\b/g],
  ['windows-user-path', /\b[A-Za-z]:[\\/]Users[\\/]([^\\/\s"'`<>]+)/g],
  ['email-address', /\b[A-Z0-9._%+-]+@([A-Z0-9.-]+\.[A-Z]{2,})\b/gi],
]

function entropy(value) {
  const counts = new Map()
  for (const character of value) counts.set(character, (counts.get(character) ?? 0) + 1)
  let result = 0
  for (const count of counts.values()) {
    const probability = count / value.length
    result -= probability * Math.log2(probability)
  }
  return result
}

function allowedSecretMatch(rule, match, file) {
  if (PLACEHOLDER.test(match[0])) return true
  if (rule === 'private-key' && /(?:^|\/)resources\/python\/.*\/test\/certdata\//.test(file)) return true
  if (rule === 'openai-style-key') {
    const body = match[0].replace(/^sk-(?:proj-|svcacct-)?/, '')
    return body.length < 32 || entropy(body) < 3.25
  }
  return false
}

function runGit(args, encoding = 'utf8') {
  const result = spawnSync('git', args, {
    encoding,
    maxBuffer: 512 * 1024 * 1024,
    windowsHide: true,
  })
  if (result.status !== 0) {
    const message = encoding === 'utf8' ? result.stderr.trim() : 'git command failed'
    throw new Error(message || `git ${args.join(' ')} failed`)
  }
  return result.stdout
}

function isText(buffer) {
  if (buffer.length > MAX_TEXT_FILE_BYTES) return false
  const sample = buffer.subarray(0, Math.min(buffer.length, 8192))
  return !sample.includes(0)
}

function lineNumber(text, offset) {
  let line = 1
  for (let index = 0; index < offset; index += 1) {
    if (text.charCodeAt(index) === 10) line += 1
  }
  return line
}

function allowedPrivacyMatch(rule, match, file) {
  if (rule === 'private-ipv4' && /(?:^|\/)network_safety\.rs$/.test(file)) {
    return true
  }
  if (rule === 'windows-user-path') {
    return /^(?:example|user|username|runneradmin|wdagtutilityaccount)$/i.test(match[1] ?? '')
  }
  if (rule === 'email-address') {
    const domain = (match[1] ?? '').toLowerCase()
    const topLevel = domain.split('.').at(-1)
    return domain === 'example.com'
      || domain === 'example.test'
      || domain.endsWith('.example.com')
      || domain === 'users.noreply.github.com'
      || ['png', 'jpg', 'jpeg', 'svg', 'webp', 'json'].includes(topLevel)
  }
  return false
}

function scanCurrent() {
  const output = runGit(['ls-files', '--cached', '--others', '--exclude-standard', '-z'], null)
  const files = output.toString('utf8').split('\0').filter(Boolean)
  const findings = []

  for (const file of files) {
    let buffer
    try {
      buffer = readFileSync(file)
    } catch {
      continue
    }
    if (!isText(buffer)) continue
    const text = buffer.toString('utf8')

    for (const [rule, expression] of secretRules) {
      expression.lastIndex = 0
      for (const match of text.matchAll(expression)) {
        if (allowedSecretMatch(rule, match, file)) continue
        findings.push({ file, line: lineNumber(text, match.index ?? 0), rule })
      }
    }

    if (/^(?:app\/package-lock\.json|app\/src-tauri\/Cargo\.lock|app\/src-tauri\/resources\/)/.test(file)) continue
    for (const [rule, expression] of privacyRules) {
      expression.lastIndex = 0
      for (const match of text.matchAll(expression)) {
        if (allowedPrivacyMatch(rule, match, file)) continue
        findings.push({ file, line: lineNumber(text, match.index ?? 0), rule })
      }
    }
  }
  return findings
}

function scanHistory() {
  const commits = runGit(['rev-list', '--all']).trim().split(/\r?\n/).filter(Boolean)
  const findings = []
  const seen = new Set()

  const highConfidenceEre = [
    '-----BEGIN (RSA |EC |OPENSSH |DSA )?PRIVATE KEY-----',
    'gh[pousr]_[A-Za-z0-9]{20,}',
    'github_pat_[A-Za-z0-9_]{20,}',
    'sk-(proj-|svcacct-)?[A-Za-z0-9_-]{32,}',
    'AIza[0-9A-Za-z_-]{30,}',
    '(AKIA|ASIA)[0-9A-Z]{16}',
    'xox[baprs]-[A-Za-z0-9-]{20,}',
    '(sk|rk)_(live|test)_[0-9A-Za-z]{20,}',
  ].join('|')

  const result = spawnSync('git', [
    'grep', '-I', '-n', '-E', '-e', highConfidenceEre,
    ...commits,
    '--', '.',
    ':(exclude)graphify-out/**',
    ':(exclude)app/src-tauri/resources/python/**',
  ], {
    encoding: 'utf8',
    maxBuffer: 64 * 1024 * 1024,
    windowsHide: true,
  })
  if (result.status !== 0 && result.status !== 1) throw new Error('git grep failed while scanning history')

  for (const line of result.stdout.split(/\r?\n/).filter(Boolean)) {
    const parsed = /^([0-9a-f]{40}):(.+?):(\d+):(.*)$/.exec(line)
    if (!parsed) continue
    const [, commit, file, , content] = parsed
    for (const [rule, expression] of secretRules) {
      expression.lastIndex = 0
      for (const match of content.matchAll(expression)) {
        if (allowedSecretMatch(rule, match, file)) continue
        const key = `${file}:${rule}`
        if (!seen.has(key)) {
          seen.add(key)
          findings.push({ commit: commit.slice(0, 12), file, rule })
        }
      }
    }
  }
  return findings
}

try {
  const findings = HISTORY ? scanHistory() : scanCurrent()
  if (findings.length > 0) {
    console.error(HISTORY ? 'History secret scan failed:' : 'Current tree secret/privacy scan failed:')
    for (const finding of findings) {
      if (HISTORY) console.error(`- commit ${finding.commit} ${finding.file} [${finding.rule}]`)
      else console.error(`- ${finding.file}:${finding.line} [${finding.rule}]`)
    }
    console.error('Matched values are intentionally not printed. Remove or rotate any real secret before pushing.')
    process.exit(1)
  }
    console.log(HISTORY ? 'History secret scan passed.' : 'Current tree secret/privacy scan passed.')
} catch (error) {
  console.error(`Secret scan could not run: ${error instanceof Error ? error.message : String(error)}`)
  process.exit(2)
}
