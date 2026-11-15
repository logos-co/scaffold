//! `.dogfooding/features/ai-skills.md`

use std::fs;
use std::path::Path;

use crate::fixtures;
use crate::harness::{check, Ctx, R};
use crate::scenarios::files_under;

const SKILLS: &[&str] = &["basecamp", "lez-template", "lgs-cli", "spel-template"];

/// Everything after the closing `---` of a frontmatter block.
fn body(text: &str) -> String {
    let mut fences = 0;
    let mut out = String::new();
    for line in text.lines() {
        if fences < 2 {
            if line == "---" {
                fences += 1;
            }
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn frontmatter(text: &str) -> String {
    text.split("---").nth(1).unwrap_or("").to_string()
}

fn check_project(cx: &Ctx, root: &Path) -> R {
    let read = |rel: &str| fs::read_to_string(root.join(rel)).unwrap_or_default();
    let skill_files = files_under(&root.join(".claude/skills"), &[]);
    let rule_files = files_under(&root.join(".cursor/rules"), &[]);
    check!(
        skill_files.len() == 4 && rule_files.len() == 4,
        "{}: expected four skills and four rules, found {skill_files:?} and {rule_files:?}",
        root.display()
    );
    let agents = read("AGENTS.md");
    let mut last_row = 0;
    for s in SKILLS {
        let canonical =
            fs::read_to_string(cx.repo.join(format!("skills/{s}/SKILL.md"))).unwrap_or_default();
        check!(!canonical.is_empty(), "repo has no skills/{s}/SKILL.md");
        check!(
            read(&format!(".claude/skills/{s}/SKILL.md")) == canonical,
            "{}: .claude/skills/{s}/SKILL.md differs from the repo copy",
            root.display()
        );
        let mdc = read(&format!(".cursor/rules/{s}.mdc"));
        let fm = frontmatter(&mdc);
        check!(
            fm.contains("description:")
                && fm.contains("alwaysApply: false")
                && !fm.contains("\nname:"),
            "{}: {s}.mdc frontmatter is wrong: {fm:?}",
            root.display()
        );
        check!(
            body(&mdc) == body(&canonical),
            "{}: {s}.mdc body differs from SKILL.md",
            root.display()
        );
        // One AGENTS.md row per skill: a link to the SKILL.md, then a description.
        let link = format!("(.claude/skills/{s}/SKILL.md)");
        let Some(pos) = agents.find(&link) else {
            return Err(crate::harness::Stop::Fail(format!(
                "{}: AGENTS.md has no row linking {link}",
                root.display()
            )));
        };
        check!(
            pos > last_row,
            "{}: AGENTS.md rows are not sorted by skill name",
            root.display()
        );
        last_row = pos;
        let row = agents[pos..].lines().next().unwrap_or("");
        let description = row.split('|').nth(1).unwrap_or("").trim();
        check!(
            !description.is_empty(),
            "{}: AGENTS.md row for {s} has no description",
            root.display()
        );
    }
    let ignore = read(".gitignore");
    check!(
        !ignore
            .lines()
            .any(|l| l.contains(".claude") || l.contains(".cursor") || l.contains("AGENTS")),
        "{}: .gitignore excludes the skills",
        root.display()
    );
    Ok(())
}

/// SKILLS-001 — New projects ship the four AI skills
pub fn new_projects_ship_skills(cx: &Ctx) -> R {
    let (main, _) = fixtures::main_created(cx, false)?;
    check_project(cx, &main.root)?;
    match fixtures::spel_created(cx, false) {
        Ok((spel, _)) => check_project(cx, &spel.root)?,
        Err(_) => cx.note("spel project not checked: the spel CLI is unavailable"),
    }
    Ok(())
}

/// SKILLS-002 — init refreshes skills idempotently and leaves team-owned ones alone
pub fn init_refresh(cx: &Ctx) -> R {
    let dir = cx.empty_dir("skills-init");
    cx.lgs(&dir, &["init"]).run().ok()?;
    let tracked = [
        "AGENTS.md",
        ".claude/skills/lgs-cli/SKILL.md",
        ".cursor/rules/lgs-cli.mdc",
    ];
    let before: Vec<String> = tracked
        .iter()
        .map(|f| fs::read_to_string(dir.join(f)).unwrap_or_default())
        .collect();
    check!(
        before.iter().all(|c| !c.is_empty()),
        "`init` did not install the skills"
    );
    fs::create_dir_all(dir.join(".claude/skills/team-notes")).expect("mkdir");
    fs::write(dir.join(".claude/skills/team-notes/SKILL.md"), "keep\n").expect("write");
    cx.lgs(&dir, &["init"])
        .run()
        .ok()?
        .has("AI skills refreshed under .claude/skills/, .cursor/rules/, AGENTS.md.")?;
    for (f, old) in tracked.iter().zip(&before) {
        check!(
            &fs::read_to_string(dir.join(f)).unwrap_or_default() == old,
            "a second `init` changed {f}"
        );
    }
    check!(
        fs::read_to_string(dir.join(".claude/skills/team-notes/SKILL.md")).unwrap_or_default()
            == "keep\n",
        "`init` touched a team-owned skill"
    );
    Ok(())
}
