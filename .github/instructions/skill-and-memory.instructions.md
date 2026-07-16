---
applyTo: "**/*"
description: "Enforce consistent SKILL.md creation, skills.md index updates, and memory persistence workflows."
---

## Trigger Cases
Apply this workflow when user intent includes:
- skill creation or missing skill activation
- unreliable agent behavior around SKILL.md
- memory persistence, memory quality, or recall issues
- tuning requests, especially repeated stress tests like 20+ or 50 prompts

## Workflow
1. Check whether `skills.md` exists. Create it if missing.
2. Ensure each managed skill has `.github/skills/<name>/SKILL.md` with clean frontmatter.
3. For memory updates, inspect `/memories/` before writing.
4. Record only reusable lessons; avoid noisy per-prompt logging.
5. Run `node scripts/validate-agent-discipline.mjs --runs 50` after customization edits.

## Quality Rules
- Use explicit trigger keywords in each skill description.
- Keep descriptions concrete: include "Use when ..." and keyword variants.
- Keep memory notes concise and deduplicated.
