import fs from 'node:fs'
import path from 'node:path'
import ts from 'typescript'

const roots = ['src', 'src-tauri', 'scripts']
const sourceExtensions = new Set(['.ts', '.tsx', '.js', '.jsx', '.mjs', '.cjs', '.rs', '.py', '.ps1'])
const ignoredDirs = new Set([
  'node_modules',
  'dist',
  'target',
  '.git',
  '.venv',
  'venv',
  '__pycache__',
  '.pytest_cache',
  'wheels',
  'gen',
  'icons',
  'resources',
])

const germanPattern = /[\u00c4\u00d6\u00dc\u00e4\u00f6\u00fc\u00df]|\b(Anlegen|Duplizieren|Allgemein|Arbeitsbereich|Berechtigung|Freigabe|Gedaechtnis|Persoenlichkeit|Aufgabe|Antwort|Frage|Bitte|Datei|Ordner|Kontext|Modell|Noch|Neu|Neue|Neuer|Neues|keine|kein|nicht|werden|wurde|konnte|fuer|ueber|checkn|auschoose|Hinzuf|Loesch|loeschen|laeuft|verfuegbar|Eintraege|Uebergabe|Uebergaben|Prozess|Speichere|Teste|Persistenz|Code-Kommentare|Task-Uebergabe|Hinweis|Eingabe|Optionale|Optionaler|Maximale|Suchanfrage|Zeichenanzahl|Prozessname|Arbeitsverzeichnis|Ungueltig|ungueltig|Konfiguration|gefunden|erstellt|geladen|gestartet|bereit|waehlen|Auswahl|Abbrechen|Speichern|Laden|Loeschen|Bearbeiten|Senden|Naechste|letzten|konkreten|aktuellen|Darstellung|Schriftgroesse|Startansicht|Projekt|Daten|Erlaubt|Zugriff|Ablehnen|Leeren|Wiederverwenden|Hauptchat|Profil|Hinweise|Benachrichtigungen|Bestaetigung|Filesicherheit|Filezugriff|Datenhaltung|Aufbewahrung|Intervall|Systemstart|Workspace-Pfad|Systemprompts|Agent-Memory|Agenten|Telemetrie|Sourcen|Ansicht|Autostart|konfigurieren|ausgeblendet|Anzeigen|Hintergrund|Standard-Workspace|Desktop-Integration|Laufzeit|Uebersicht|Vertraege|Verhalten|Laeufe|parallele|Versuche|geteilte|Resultse|sofort|stoppen|Vorherige|Globale|Gilt|automatisch|ohne|eigenes|Zugriffe|Verbindungen|Fokus|runtimeverhalten|Werkzeuge|angelegt|oeffnen|umgeschaltet|volle|Suche|gesetzt|Ausfuehren)\b/i

const uiPropNames = new Set([
  'label',
  'title',
  'description',
  'placeholder',
  'aria-label',
  'alt',
  'message',
  'content',
  'subtitle',
  'hint',
  'emptyText',
  'confirmLabel',
  'cancelLabel',
  'submitLabel',
  'buttonLabel',
  'tooltip',
  'helperText',
  'notice',
  'summary',
  'heading',
  'text',
  'question',
])

function walkFiles(dir, out = []) {
  if (!fs.existsSync(dir)) return out
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    if (entry.isDirectory()) {
      if (!ignoredDirs.has(entry.name)) walkFiles(path.join(dir, entry.name), out)
      continue
    }
    const file = path.join(dir, entry.name)
    if (sourceExtensions.has(path.extname(entry.name))) out.push(file)
  }
  return out
}

function isTestFile(file) {
  return /\.test\.[cm]?[tj]sx?$/.test(file) || /\.spec\.[cm]?[tj]sx?$/.test(file)
}

function isAllowedGermanMatch(file, line) {
  const normalized = file.replace(/\\/g, '/')
  if (normalized === 'scripts/i18n-audit.mjs') {
    return true
  }
  if (normalized === 'src/engine/core/queryEngine.ts') {
    return /(sortier|bildschirm|ich werde|werde ich|n\u00e4chstes|nächstes|oeffne|öffne|schlie|freigabe|destruktiv|riskant|prozess|dateisystem|konsole|verzeichnis)/i.test(line)
  }
  if (normalized === 'src/components/CoworkView.tsx') {
    return /\b(eine|einer|eines)\b|\boptionen\b|\bauswahl\b/.test(line)
  }
  if (normalized === 'src/engine/tools/registry.ts') {
    return /\b(eine|einer|eines)\b|\boptionen\b|\bauswahl\b/.test(line)
  }
  if (normalized === 'src/engine/crew/workTaskCrewRuntime.ts') {
    return /(?:RESEARCH|PRESENTATION)_TASK_PATTERN/.test(line)
  }
  if (normalized === 'src/engine/memory/memorySystem.ts') {
    return /(?:explicitMatch|isPreference|isReusableFact)/.test(line)
  }
  if (normalized === 'src-tauri/src/scheduler.rs') {
    return /\.replace\('\u00e4'|\.replace\('\u00f6'|\.replace\('\u00fc'/.test(line)
  }
  if (normalized === 'src/utils/attachmentPromptContext.ts') {
    return /'wenn'|'bitte'|'durch'|alle\\s\+datei|dateiliste|allen\\s\+dateien/.test(line)
  }
  if (normalized === 'src/utils/followUpPrompt.ts') {
    return /bitte geben sie an/.test(line)
  }
  if (normalized === 'src/utils/chatAttachments.ts') {
    return /ordner|File-Metadaten|Retrieval-Context/.test(line)
  }
  if (normalized === 'src/utils/webSearchSources.ts') {
    return /Web-Suche failed/.test(line)
  }
  return false
}

function readResources() {
  const file = 'src/i18n.ts'
  const source = fs.readFileSync(file, 'utf8')
  const sf = ts.createSourceFile(file, source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TS)
  let expr = null
  function visit(node) {
    if (ts.isVariableDeclaration(node) && ts.isIdentifier(node.name) && node.name.text === 'resources') {
      let init = node.initializer
      if (init && ts.isAsExpression(init)) init = init.expression
      expr = init?.getText(sf) ?? null
    }
    ts.forEachChild(node, visit)
  }
  visit(sf)
  if (!expr) throw new Error('Could not find i18n resources')
  return Function(`return (${expr})`)()
}

function nameOf(node) {
  if (!node) return ''
  if (ts.isIdentifier(node) || ts.isStringLiteral(node)) return node.text
  return ''
}

function collectTsFindings(files) {
  const rawUiStrings = []
  const usedTranslationKeys = new Set()

  for (const file of files.filter((item) => /\.(ts|tsx|mts|cts)$/.test(item) && !isTestFile(item))) {
    const source = fs.readFileSync(file, 'utf8')
    const sf = ts.createSourceFile(
      file,
      source,
      ts.ScriptTarget.Latest,
      true,
      file.endsWith('.tsx') ? ts.ScriptKind.TSX : ts.ScriptKind.TS,
    )

    function addRawUi(node, kind, value) {
      const text = String(value ?? '').trim()
      if (!text || !germanPattern.test(text)) return
      const pos = sf.getLineAndCharacterOfPosition(node.getStart(sf))
      rawUiStrings.push({ file, line: pos.line + 1, kind, value: text.slice(0, 180) })
    }

    function callName(expr) {
      if (ts.isIdentifier(expr)) return expr.text
      if (ts.isPropertyAccessExpression(expr)) return expr.name.text
      return ''
    }

    function visit(node) {
      if (ts.isCallExpression(node)) {
        const called = callName(node.expression)
        const first = node.arguments[0]
        if ((called === 'tr' || called === 't') && first && ts.isStringLiteralLike(first)) {
          usedTranslationKeys.add(first.text)
        }
        if (ts.isPropertyAccessExpression(node.expression) && node.expression.expression.getText(sf) === 'i18n' && called === 't' && first && ts.isStringLiteralLike(first)) {
          usedTranslationKeys.add(first.text)
        }
      }

      if (!isTestFile(file)) {
        if (ts.isJsxText(node)) {
          addRawUi(node, 'jsx-text', node.getFullText().replace(/\s+/g, ' '))
        }
        if (ts.isJsxAttribute(node)) {
          const prop = nameOf(node.name)
          if (uiPropNames.has(prop) && node.initializer && ts.isStringLiteral(node.initializer)) {
            addRawUi(node.initializer, `jsx-attr:${prop}`, node.initializer.text)
          }
        }
        if (ts.isPropertyAssignment(node)) {
          const prop = nameOf(node.name)
          if (uiPropNames.has(prop) && ts.isStringLiteralLike(node.initializer)) {
            addRawUi(node.initializer, `object-prop:${prop}`, node.initializer.text)
          }
        }
      }

      ts.forEachChild(node, visit)
    }

    visit(sf)
  }

  return { rawUiStrings, usedTranslationKeys }
}

const files = roots.flatMap((root) => walkFiles(root))
const sourceGermanFindings = []
const testGermanFindings = []

for (const file of files) {
  if (file.replace(/\\/g, '/') === 'src/i18n.ts') continue
  const lines = fs.readFileSync(file, 'utf8').split(/\r?\n/)
  lines.forEach((line, index) => {
    const lineWithoutTranslationKeys = line
      .replace(/\b(?:tr|t)\(\s*(["'`])(?:\\.|(?!\1).)*\1/g, '')
      .replace(/\/\/.*/, '')
    if (!germanPattern.test(lineWithoutTranslationKeys)) return
    const finding = { file, line: index + 1, text: line.trim().slice(0, 220) }
    if (isTestFile(file)) {
      testGermanFindings.push(finding)
      return
    }
    if (!isAllowedGermanMatch(file, line)) sourceGermanFindings.push(finding)
  })
}

const resources = readResources()
const en = resources.en.translation
const de = resources.de.translation
const enKeys = Object.keys(en).sort()
const deKeys = Object.keys(de).sort()
const { rawUiStrings, usedTranslationKeys } = collectTsFindings(files)
const missingInDe = enKeys.filter((key) => !(key in de))
const missingInEn = deKeys.filter((key) => !(key in en))
const germanInEnglishResources = enKeys
  .filter((key) => germanPattern.test(String(en[key])))
  .map((key) => ({ key, value: String(en[key]).slice(0, 220) }))
const missingUsedKeys = [...usedTranslationKeys]
  .filter((key) => !(key in en) || !(key in de))
  .sort()

const summary = {
  resources: {
    enKeys: enKeys.length,
    deKeys: deKeys.length,
    missingInEn: missingInEn.length,
    missingInDe: missingInDe.length,
    germanInEnglishResources: germanInEnglishResources.length,
    missingUsedKeys: missingUsedKeys.length,
  },
  sourceGermanFindings: sourceGermanFindings.length,
  rawUiStrings: rawUiStrings.length,
  testGermanFindings: testGermanFindings.length,
  samples: {
    sourceGermanFindings: sourceGermanFindings.slice(0, 30),
    rawUiStrings: rawUiStrings.slice(0, 30),
    germanInEnglishResources: germanInEnglishResources.slice(0, 30),
    missingUsedKeys: missingUsedKeys.slice(0, 30),
    testGermanFindings: testGermanFindings.slice(0, 30),
  },
}

console.log(JSON.stringify(summary, null, 2))

if (
  missingInEn.length ||
  missingInDe.length ||
  germanInEnglishResources.length ||
  missingUsedKeys.length ||
  sourceGermanFindings.length ||
  rawUiStrings.length
) {
  process.exit(1)
}
