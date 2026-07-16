#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";

const args = process.argv.slice(2);
const runsArgIndex = args.indexOf("--runs");
const runs = runsArgIndex >= 0 ? Number.parseInt(args[runsArgIndex + 1] || "50", 10) : 50;
const checkNotes = args.includes("--check-notes");
const writeTasks = args.includes("--write-tasks");
const tasksOutIndex = args.indexOf("--tasks-out");
const tasksOut = tasksOutIndex >= 0 ? args[tasksOutIndex + 1] : "tasks/agent-memory-prompts.md";
const memoryPathArgIndex = args.indexOf("--memory-file");
const memoryFilePath = memoryPathArgIndex >= 0 ? args[memoryPathArgIndex + 1] : null;

if (!Number.isInteger(runs) || runs <= 0) {
  console.error("Invalid --runs value. Use a positive integer.");
  process.exit(1);
}

const root = process.cwd();
const requiredPaths = [
  ".github/copilot-instructions.md",
  ".github/instructions/skill-and-memory.instructions.md",
  ".github/skills/memory-and-skill-discipline/SKILL.md",
  "skills.md",
];

const missing = requiredPaths.filter((p) => !fs.existsSync(path.join(root, p)));
if (missing.length > 0) {
  console.error("Missing required files:");
  for (const item of missing) console.error(`- ${item}`);
  process.exit(1);
}

const skillPath = path.join(root, ".github/skills/memory-and-skill-discipline/SKILL.md");
const skillText = fs.readFileSync(skillPath, "utf8");

function extractFrontmatter(text) {
  const match = text.match(/^---\r?\n([\s\S]*?)\r?\n---\r?\n/);
  if (!match) return null;
  return match[1];
}

const frontmatter = extractFrontmatter(skillText);
if (!frontmatter) {
  console.error("SKILL.md frontmatter is missing or malformed.");
  process.exit(1);
}

if (!/\n?name:\s*memory-and-skill-discipline\s*(\n|$)/.test(frontmatter)) {
  console.error("SKILL.md frontmatter is missing expected 'name'.");
  process.exit(1);
}
if (!/\n?description:\s*"[\s\S]+"\s*(\n|$)/.test(frontmatter)) {
  console.error("SKILL.md frontmatter is missing a quoted 'description'.");
  process.exit(1);
}

const descLine = frontmatter.split("\n").find((line) => line.trim().startsWith("description:")) || "";
const descLower = descLine.toLowerCase();

const requiredKeywords = ["skill", "skills.md", "skill.md", "memory", "gedaechtnis", "speichern", "tuning", "reliability"];
const missingKeywords = requiredKeywords.filter((k) => !descLower.includes(k));
if (missingKeywords.length > 0) {
  console.error("Description is missing trigger keywords:");
  for (const k of missingKeywords) console.error(`- ${k}`);
  process.exit(1);
}

const promptTemplates = [
  "Bitte leg eine skill datei an und speichere es im gedaechtnis.",
  "Warum wird skills.md nicht gepflegt?",
  "Mach den Agent reliable fuer memory persistence.",
  "Tuning fuer 50 prompts zur skill erkennung.",
  "SKILL.md wird ignoriert, bitte fixen.",
  "Speicher wichtige Infos dauerhaft im memory.",
  "Agent Customization fuer skills und memory.",
  "Bitte teste die reliability mit wiederholten Requests und tuning.",
  "Index skills.md fehlt, bitte automatisch pflegen.",
  "Wie erzwinge ich Skill-Discovery ueber Keywords?"
];

const expandedPrompts = Array.from({ length: runs }, (_, i) => `${promptTemplates[i % promptTemplates.length]} [run ${i + 1}]`);

const taskTemplates = [
  "Aufgabe: Lege skills.md an falls fehlt und notiere das Ergebnis im Gedaechtnis.",
  "Aufgabe: Erstelle SKILL.md fuer memory handling und speichere 1 Notiz dazu.",
  "Aufgabe: Pruefe Memory-Duplikate und speichere nur den deduplizierten Kernpunkt.",
  "Aufgabe: Simuliere 3 User-Requests und notiere wiederverwendbare Erkenntnisse.",
  "Aufgabe: Fuehre eine kleine Reliability-Pruefung aus und speichere Lessons Learned.",
  "Aufgabe: Aktualisiere skills.md Index und hinterlege eine kurze Memory-Notiz.",
  "Aufgabe: Teste Skill Trigger Keywords und notiere erkannte Luecken.",
  "Aufgabe: Erzeuge einen Prompt fuer Skill-Aufruf und speichere Resultat im Memory.",
  "Aufgabe: Pruefe, ob vor Memory-Write immer Memory-Read passiert; notiere Befund.",
  "Aufgabe: Tune die Skill-Beschreibung fuer bessere Discovery und speichere Delta."
];

const taskRuns = Array.from({ length: runs }, (_, i) => {
  const id = String(i + 1).padStart(3, "0");
  const task = taskTemplates[i % taskTemplates.length];
  return {
    id: `TASK-${id}`,
    prompt: `${task} [${id}]`,
  };
});

function writeTaskPromptFile(outPath, tasks) {
  const abs = path.resolve(root, outPath);
  fs.mkdirSync(path.dirname(abs), { recursive: true });
  const lines = [
    "# Agent Memory Test Tasks",
    "",
    "Use these prompts one by one with your agent.",
    "Requirement: after each task, the agent must write at least one concise memory note referencing the task ID.",
    "",
    "## Prompts",
    "",
  ];
  for (const t of tasks) {
    lines.push(`- ${t.id}: ${t.prompt}`);
  }
  lines.push("", "## Verification rule", "", "Each memory note must contain the corresponding TASK-XXX id.");
  fs.writeFileSync(abs, lines.join("\n"), "utf8");
  return abs;
}

function verifyTaskNotes(tasks, rawText) {
  const text = (rawText || "").toLowerCase();
  const missing = [];
  for (const t of tasks) {
    if (!text.includes(t.id.toLowerCase())) {
      missing.push(t.id);
    }
  }
  return missing;
}

function hitsAnyKeyword(text, keywords) {
  const lower = text.toLowerCase();
  return keywords.some((k) => lower.includes(k));
}

let covered = 0;
for (const prompt of expandedPrompts) {
  if (hitsAnyKeyword(prompt, requiredKeywords)) {
    covered += 1;
  }
}

const coverage = covered / expandedPrompts.length;
const threshold = 0.95;

console.log(`Runs: ${expandedPrompts.length}`);
console.log(`Keyword coverage: ${(coverage * 100).toFixed(1)}%`);

if (coverage < threshold) {
  console.error(`Coverage below threshold ${(threshold * 100).toFixed(0)}%.`);
  process.exit(1);
}

if (writeTasks) {
  const out = writeTaskPromptFile(tasksOut, taskRuns);
  console.log(`Task prompt file written: ${out}`);
}

if (checkNotes) {
  if (!memoryFilePath) {
    console.error("Missing --memory-file <path> for note verification.");
    process.exit(1);
  }
  const absMemory = path.resolve(root, memoryFilePath);
  if (!fs.existsSync(absMemory)) {
    console.error(`Memory file not found: ${absMemory}`);
    process.exit(1);
  }
  const memoryText = fs.readFileSync(absMemory, "utf8");
  const missingTaskNotes = verifyTaskNotes(taskRuns, memoryText);
  if (missingTaskNotes.length > 0) {
    console.error("Missing task note references for:");
    for (const id of missingTaskNotes) console.error(`- ${id}`);
    process.exit(1);
  }
  console.log("Task note verification passed.");
}

console.log("Validation passed.");
