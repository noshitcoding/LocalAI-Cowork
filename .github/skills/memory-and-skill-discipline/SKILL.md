---
name: memory-and-skill-discipline
description: "Use when users ask for reliable SKILL.md creation, skills.md maintenance, memory persistence, or tuning with repeated requests (for example 50 prompts). Keywords: skill, skills.md, SKILL.md, memory, gedaechtnis, speichern, tuning, reliability."
---

# Memory And Skill Discipline

## Purpose
Provide a repeatable workflow that makes skill handling and memory persistence predictable.

## Steps
1. Inspect existing customization files (`.github/copilot-instructions.md`, `.github/instructions/*.instructions.md`, `.github/skills/*/SKILL.md`).
2. If a requested skill is missing, create `.github/skills/<slug>/SKILL.md` with clear trigger keywords in `description`.
3. Update `skills.md` so humans can see what exists and when it should be used.
4. Before writing memory notes, inspect `/memories/` to avoid duplicates.
5. Save only high-signal reusable facts.
6. Run `node scripts/validate-agent-discipline.mjs --runs 50` and fix failing checks.

## Definition Of Done
- Skill exists and is discoverable by description keywords.
- `skills.md` references the skill and purpose.
- Memory note is updated when a reusable lesson is found.
- Validation script reports success for configured runs.
