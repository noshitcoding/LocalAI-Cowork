import { describe, expect, it } from 'vitest'
import { extractAttachmentsFromContent, getAttachmentDisplayName, isImageAttachment, isImageAttachmentPath, toImageContentBlock } from './chatAttachments'

describe('extractAttachmentsFromContent', () => {
  it('extracts files and folders from prompt content and strips the block', () => {
    const parsed = extractAttachmentsFromContent([
      'Bitte analysiere dieses Projekt.',
      '',
      'Verbundene Pfade (2):',
      '1. Datei: C:\\workspace\\notes.txt',
      '2. Ordner: C:\\workspace\\src',
    ].join('\n'))

    expect(parsed.content).toBe('Bitte analysiere dieses Projekt.')
    expect(parsed.attachments).toEqual([
      { path: 'C:\\workspace\\notes.txt', kind: 'file' },
      { path: 'C:\\workspace\\src', kind: 'folder' },
    ])
  })

  it('leaves normal content untouched when no attachment block exists', () => {
    const parsed = extractAttachmentsFromContent('Nur ein normaler Prompt.')

    expect(parsed.content).toBe('Nur ein normaler Prompt.')
    expect(parsed.attachments).toEqual([])
  })

  it('strips generated metadata and retrieval blocks from augmented prompts', () => {
    const parsed = extractAttachmentsFromContent([
      'sortiere den inhalt abweschselnd in zwei ordner a und b um',
      '',
      'Verbundene Pfade (1):',
      '1. Ordner: C:\\workspace\\javastuff',
      '',
      'Datei-Metadaten (ohne Volltext):',
      '- Ordner C:\\workspace\\javastuff | Dateien gesamt 11 | betrachtet 11',
      '',
      'Retrieval-Kontext (selektiv gelesen):',
      'Selektierte Kandidaten (Ranking):',
      '1. Film.java | Score 9 | Sprache Java | Ordner-Anhang C:\\workspace\\javastuff',
      '',
      'Nicht analysierbare Anhaenge:',
      '- C:\\workspace\\javastuff\\Film.java: format does not provide text extraction',
    ].join('\n'))

    expect(parsed.content).toBe('sortiere den inhalt abweschselnd in zwei ordner a und b um')
    expect(parsed.attachments).toEqual([
      { path: 'C:\\workspace\\javastuff', kind: 'folder' },
    ])
  })
})

describe('isImageAttachmentPath', () => {
  it('detects common image file extensions case-insensitively', () => {
    expect(isImageAttachmentPath('C:\\workspace\\Screenshot.PNG')).toBe(true)
    expect(isImageAttachmentPath('/tmp/photo.jpeg?version=2')).toBe(true)
  })

  it('ignores non-image paths and folders', () => {
    expect(isImageAttachmentPath('C:\\workspace\\notes.txt')).toBe(false)
    expect(isImageAttachmentPath('C:\\workspace\\folder')).toBe(false)
  })
})

describe('inline image attachments', () => {
  it('prefers the attachment label and detects inline image media types', () => {
    const attachment = {
      path: 'clipboard-image-1.png',
      kind: 'file' as const,
      label: 'clipboard-screenshot.png',
      mediaType: 'image/png',
      dataUrl: 'data:image/png;base64,QUJD',
      source: 'inline' as const,
    }

    expect(getAttachmentDisplayName(attachment)).toBe('clipboard-screenshot.png')
    expect(isImageAttachment(attachment)).toBe(true)
  })

  it('converts inline data urls into engine image blocks', async () => {
    const block = await toImageContentBlock({
      path: 'clipboard-image-1.png',
      kind: 'file',
      mediaType: 'image/png',
      dataUrl: 'data:image/png;base64,QUJD',
      source: 'inline',
    })

    expect(block).toEqual({
      type: 'image',
      source: {
        type: 'base64',
        media_type: 'image/png',
        data: 'QUJD',
      },
    })
  })
})
