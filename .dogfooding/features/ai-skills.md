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
    title: init refreshes skills idempotently and leaves team-owned ones alone
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 E3
retired: [SKILLS-003]
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
files, each byte-identical to `skills/<name>/SKILL.md` in the repo, and the same four
`.cursor/rules/<name>.mdc` files: frontmatter with `description:` and `alwaysApply: false`,
no `name:`, and a body identical to SKILL.md. `AGENTS.md` has one table row per skill, sorted
by name: the name as a link to `.claude/skills/<name>/SKILL.md`, then the description from
the SKILL.md frontmatter. `.gitignore` does not exclude `.claude/`, `.cursor/` or `AGENTS.md`.

### Preconditions
A default and a spel project that nobody edited: reuse CREATE-001's and SPEL-001's, or create
two. Set `PROJECTS="<default-project> <spel-project>"`.

### Steps
1. `for p in $PROJECTS; do find $p/.claude/skills $p/.cursor/rules -type f | sort; grep -c 'claude\|cursor\|AGENTS' $p/.gitignore; done`
2. Compare every skill and rule with the repo copy (the `.mdc` and SKILL.md bodies both start
   after their second `---` line):
   ```bash
   body() { awk 'f{print} /^---$/{c++; if(c==2)f=1}' "$1"; }
   for p in $PROJECTS; do for s in basecamp lez-template lgs-cli spel-template; do
     diff $p/.claude/skills/$s/SKILL.md "$REPO_ROOT/skills/$s/SKILL.md" &&
     diff <(body $p/.cursor/rules/$s.mdc) <(body "$REPO_ROOT/skills/$s/SKILL.md") && echo "$p $s identical"
     head -5 $p/.cursor/rules/$s.mdc | grep -c '^name:'
   done; done
   ```
3. `for p in $PROJECTS; do grep -F '(.claude/skills/' "$p/AGENTS.md"; ( cd "$p" && grep -o '(\.claude/skills/[^)]*)' AGENTS.md | tr -d '()' | xargs ls ); done`

### Success criteria
- Step 1 lists four SKILL.md and four .mdc files per project, and the `grep -c` prints `0`.
- Step 2 prints `identical` for all eight project/skill pairs, and `0` for every `name:` count.
- Step 3 prints four rows per project (`basecamp`, `lez-template`, `lgs-cli`,
  `spel-template`), each with a non-empty description, and every linked SKILL.md exists.

## SKILLS-002 — init refreshes skills idempotently and leaves team-owned ones alone

### Expected
`init` in a bare directory installs the same skills and `AGENTS.md`. Re-running `init`
succeeds, prints `AI skills refreshed under .claude/skills/, .cursor/rules/, AGENTS.md.`, and
leaves the content byte-identical. `init` owns only the four shipped skill names: other
directories under `.claude/skills/` are not touched.

### Preconditions
A directory adopted with `init` (CREATE-007's `adopt` works, or `mkdir d && cd d && lgs init`).

### Steps
1. `shasum AGENTS.md .claude/skills/lgs-cli/SKILL.md .cursor/rules/lgs-cli.mdc > /tmp/s1`
2. `mkdir -p .claude/skills/team-notes && echo keep > .claude/skills/team-notes/SKILL.md`
3. `"$SCAFFOLD_BIN" init && shasum -c /tmp/s1 && cat .claude/skills/team-notes/SKILL.md`

### Success criteria
- Step 3 exits 0 with the refreshed message, every checksum is `OK`, and it prints `keep`.
