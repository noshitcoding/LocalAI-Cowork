const WEEKDAY_PREFIXES = new Set([
  'montag',
  'dienstag',
  'mittwoch',
  'donnerstag',
  'freitag',
  'samstag',
  'sonntag',
  'monday',
  'tuesday',
  'wednesday',
  'thursday',
  'friday',
  'saturday',
  'sunday',
])

export const SCHEDULE_HELP_TEXT = 'Nutze: /schedule <ausdruck> <aufgabe>\nBeispiele: /schedule daily 09:00 Taeglicher Report | /schedule every 30 min Repository pruefen | /schedule every 1h Inbox pruefen | /schedule monday 08:30 Weekly Sync'

export type ParsedScheduleInput = {
  scheduleExpr: string
  prompt: string
}

export function parseScheduledTaskInput(input: string): ParsedScheduleInput | null {
  const trimmed = input.trim()
  if (!trimmed) return null

  const tokens = trimmed.split(/\s+/)
  if (tokens.length < 3) return null

  const first = tokens[0].toLowerCase()
  let scheduleTokenCount = 0

  if (first === 'every' || first === 'alle') {
    if (tokens.length >= 3 && /^\d+(min|m|h|hr|hrs|hour|hours|std|stunde|stunden)$/i.test(tokens[1])) {
      scheduleTokenCount = 2
    } else if (tokens.length >= 4) {
      scheduleTokenCount = 3
    } else {
      return null
    }
  } else if ((first === 'daily' || first === 'taeglich') && tokens.length >= 3) {
    scheduleTokenCount = 2
  } else if (WEEKDAY_PREFIXES.has(first) && tokens.length >= 3) {
    scheduleTokenCount = 2
  } else {
    return null
  }

  const scheduleExpr = tokens.slice(0, scheduleTokenCount).join(' ')
  const prompt = tokens.slice(scheduleTokenCount).join(' ').trim()

  if (!prompt) return null

  return { scheduleExpr, prompt }
}

export function parseBackendDate(value: string | null | undefined): number | null {
  if (!value) return null
  const timestamp = Date.parse(value)
  return Number.isFinite(timestamp) ? timestamp : null
}