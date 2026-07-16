# Copilot Project Instructions

## Goal
Make skill creation and memory persistence reliable and repeatable.

## Required Behavior
- If the user asks about skill setup, skill reliability, agent customization, or memory persistence, ensure a skill exists at `.github/skills/<skill-name>/SKILL.md`.
- Keep a workspace index in `skills.md`. If a skill is added or renamed, update the index in the same change.
- For memory operations, always inspect `/memories/` first to avoid duplicates, then create or update concise notes.
- Prefer updating existing memory notes over creating many small files.
- Keep memory entries short, factual, and reusable.

## Completion Checklist
- Skill file exists and has valid YAML frontmatter (`name`, `description`).
- `skills.md` contains the skill and usage intent.
- Memory note was updated when a reusable lesson was discovered.
- Validation command passes: `node scripts/validate-agent-discipline.mjs --runs 50`
