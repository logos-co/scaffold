---
feature: ai-skills
prefix: SKILLS
surface: cli
scenarios:
  - id: SKILLS-001
    title: New projects ship the four AI skills
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 E3
  - id: SKILLS-002
    title: init installs skills and re-runs idempotently
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 E3
  - id: SKILLS-003
    title: init leaves team-owned skills alone
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 E3
    depends_on: [SKILLS-002]
retired: []
---

# AI skills

Every generated or adopted project gets the canonical skill set from the repo's `skills/`:
`lgs-cli`, `lez-template`, `spel-template` and `basecamp`. They go into
`.claude/skills/<name>/SKILL.md` and `.cursor/rules/<name>.mdc`, with an `AGENTS.md` index.
All of it is version-controlled, never gitignored. Related:
[Project creation](project-creation.md).

## SKILLS-001 — New projects ship the four AI skills

### Expected
Default and spel projects both contain exactly the four `.claude/skills/<name>/SKILL.md`
files, each byte-identical to `skills/<name>/SKILL.md` in the repo. They also contain the
same four `.cursor/rules/<name>.mdc` files: frontmatter with `description:` and
`alwaysApply: false`, no `name:`, and a body identical to SKILL.md. `AGENTS.md` has one table
row per skill, sorted by name. The first cell is the skill name as a link to
`.claude/skills/<name>/SKILL.md`, and the second is the description from the SKILL.md
frontmatter. `.gitignore`
does not exclude `.claude/`, `.cursor/` or `AGENTS.md`.

### Preconditions
None (no setup or network needed beyond creation).

### Steps
1. `"$SCAFFOLD_BIN" new dogfood-skills-default` and `"$SCAFFOLD_BIN" new dogfood-skills-spel --template spel`
2. `find dogfood-skills-*/.claude/skills dogfood-skills-*/.cursor/rules -type f | sort`
3. `diff dogfood-skills-default/.claude/skills/lgs-cli/SKILL.md "$REPO_ROOT/skills/lgs-cli/SKILL.md"`
4. `head -5 dogfood-skills-default/.cursor/rules/lgs-cli.mdc; cat dogfood-skills-default/.gitignore`
5. `for p in dogfood-skills-default dogfood-skills-spel; do grep -F '(.claude/skills/' "$p/AGENTS.md"; ( cd "$p" && grep -o '(\.claude/skills/[^)]*)' AGENTS.md | tr -d '()' | xargs ls ); done`

### Success criteria
- There are four SKILL.md files and four .mdc files per project, and the diff is empty.
- The .mdc frontmatter has no `name:`, and `.gitignore` has no skill paths.
- Step 5 prints four rows per project (`basecamp`, `lez-template`, `lgs-cli`,
  `spel-template`), each with a non-empty description, and every linked SKILL.md exists.

## SKILLS-002 — init installs skills and re-runs idempotently

### Expected
`init` in a bare directory installs the same skills and `AGENTS.md`. Re-running `init`
succeeds, prints `AI skills refreshed under .claude/skills/, .cursor/rules/, AGENTS.md.`, and
leaves the content byte-identical.

### Preconditions
An empty directory.

### Steps
1. `mkdir dogfood-skills-init && cd dogfood-skills-init && "$SCAFFOLD_BIN" init`
2. `shasum AGENTS.md .claude/skills/lgs-cli/SKILL.md .cursor/rules/lgs-cli.mdc > /tmp/s1`
3. `"$SCAFFOLD_BIN" init && shasum AGENTS.md .claude/skills/lgs-cli/SKILL.md .cursor/rules/lgs-cli.mdc | diff /tmp/s1 -`

### Success criteria
- The second `init` exits 0 with the refreshed message, and the diff is empty.

## SKILLS-003 — init leaves team-owned skills alone

### Expected
`init` owns only the four shipped skill names. Other directories under `.claude/skills/` are
not touched.

### Preconditions
SKILLS-002.

### Steps
1. `mkdir -p .claude/skills/team-notes && echo keep > .claude/skills/team-notes/SKILL.md`
2. `"$SCAFFOLD_BIN" init && cat .claude/skills/team-notes/SKILL.md`

### Success criteria
- Step 2 prints `keep`.
