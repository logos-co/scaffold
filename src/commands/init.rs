use std::env;
use std::fs;
use std::path::Path;

use anyhow::{bail, Context};
use toml_edit::DocumentMut;

use crate::config::{
    default_basecamp_repo, default_lez_repo, default_lgpm_repo, default_spel_repo, serialize_config,
};
use crate::constants::{
    DEFAULT_BASECAMP_PIN, DEFAULT_FRAMEWORK_IDL_PATH, DEFAULT_FRAMEWORK_IDL_SPEC,
    DEFAULT_FRAMEWORK_VERSION, DEFAULT_LEZ, DEFAULT_LGPM_PIN, DEFAULT_SPEL, FRAMEWORK_KIND_DEFAULT,
    SCAFFOLD_TOML_SCHEMA_VERSION,
};
use crate::migrate::migrate_to_v0_2_0;
use crate::model::{Config, FrameworkConfig, FrameworkIdlConfig, LocalnetConfig, RunConfig};
use crate::state::write_text_atomic;
use crate::template::project::ensure_scaffold_in_gitignore;
use crate::template::skills::apply_skills;
use crate::DynResult;

pub(crate) fn cmd_init(bin_name: &str, dry_run: bool, no_backup: bool) -> DynResult<()> {
    let cwd = env::current_dir()?;
    cmd_init_at(&cwd, bin_name, dry_run, no_backup)
}

pub(crate) fn cmd_init_at(
    target: &Path,
    bin_name: &str,
    dry_run: bool,
    no_backup: bool,
) -> DynResult<()> {
    let scaffold_path = target.join("scaffold.toml");
    if scaffold_path.exists() {
        let existing = fs::read_to_string(&scaffold_path).with_context(|| {
            format!(
                "reading existing scaffold.toml at {}",
                scaffold_path.display()
            )
        })?;
        let mut doc: DocumentMut = existing.parse().with_context(|| {
            format!(
                "parsing existing scaffold.toml at {}",
                scaffold_path.display()
            )
        })?;

        let report = migrate_to_v0_2_0(&mut doc)?;
        let migrated = !report.changes.is_empty();

        if migrated {
            let backup_path = scaffold_path.with_extension("toml.bak");
            // If a previous migration (or a hand-curated backup) already wrote
            // scaffold.toml.bak, `fs::copy` would silently overwrite it and the
            // user would lose the older backup with no warning. Refuse instead
            // and surface both ways out: rename/delete the existing .bak, or
            // pass --no-backup to skip the backup entirely.
            let backup_collision = !no_backup && backup_path.exists();
            if dry_run {
                println!(
                    "dry-run: would migrate scaffold.toml at {} to schema v{} (no changes made)",
                    target.display(),
                    SCAFFOLD_TOML_SCHEMA_VERSION,
                );
                if !no_backup {
                    if backup_collision {
                        println!(
                            "dry-run: WOULD ABORT — backup target already exists at {}. \
                             Move/delete it, or re-run with --no-backup to skip the backup.",
                            backup_path.display(),
                        );
                    } else {
                        println!(
                            "dry-run: would write backup of current scaffold.toml to {}",
                            backup_path.display(),
                        );
                    }
                }
                for change in &report.changes {
                    println!("  - {change}");
                }
                if let Some(hint) = &report.hand_edit_hint {
                    println!("  ! {hint}");
                }
                println!(
                    "Re-run without --dry-run to apply, or `{bin_name} init --no-backup` to skip the .bak."
                );
                return Ok(());
            }

            if backup_collision {
                bail!(
                    "refusing to overwrite existing scaffold.toml.bak at {}.\n\
                     Move or delete it first, or re-run with --no-backup to migrate without writing a backup.\n\
                     Preview the migration with `{bin_name} init --dry-run`.",
                    backup_path.display(),
                );
            }

            // Create the .scaffold/ directories before rewriting scaffold.toml.
            // The fresh-init branch does the same — both paths should leave the
            // project in the same fully-initialized state, otherwise a re-run on
            // a project that was upgraded by migration alone would look wedged.
            create_scaffold_dirs(target)?;

            // Write the backup before rewriting scaffold.toml, so a crash mid
            // write can't leave both the original and the migration unrecoverable.
            if !no_backup {
                fs::copy(&scaffold_path, &backup_path).with_context(|| {
                    format!(
                        "writing backup of scaffold.toml to {} before migrating",
                        backup_path.display()
                    )
                })?;
            }
            write_text_atomic(&scaffold_path, &doc.to_string())?;
            ensure_scaffold_in_gitignore(target)?;
            println!(
                "scaffold.toml in {} migrated to schema v{}.",
                target.display(),
                SCAFFOLD_TOML_SCHEMA_VERSION,
            );
            if !no_backup {
                println!("  backup: {}", backup_path.display());
            }
            for change in report.changes {
                println!("  - {change}");
            }
            if let Some(hint) = report.hand_edit_hint {
                println!("  ! {hint}");
            }
        } else {
            // Already at current schema. Re-run is the user-facing entry point
            // for refreshing the shipped AI skills, and also recovers from a
            // wedged init where scaffold.toml landed but the .scaffold/ dirs
            // never got created.
            if dry_run {
                println!(
                    "dry-run: scaffold.toml at {} is already at schema v{}; would ensure .scaffold/state and .scaffold/logs and refresh AI skills (no changes made)",
                    target.display(),
                    SCAFFOLD_TOML_SCHEMA_VERSION,
                );
                println!(
                    "dry-run: would append `.scaffold` to {}",
                    target.join(".gitignore").display(),
                );
                return Ok(());
            }
            create_scaffold_dirs(target)?;
            ensure_scaffold_in_gitignore(target)?;
            println!(
                "scaffold.toml at {} is already at schema v{}.",
                target.display(),
                SCAFFOLD_TOML_SCHEMA_VERSION,
            );
        }

        apply_skills(target)?;
        println!("AI skills refreshed under .claude/skills/, .cursor/rules/, AGENTS.md.");

        if migrated {
            println!("Run `{bin_name} setup` to clone and build per the new schema.");
        }
        return Ok(());
    }

    // Fresh init — schema 0.2.0 by construction. Create the .scaffold/
    // directories first so that a failure here leaves no scaffold.toml
    // behind and a re-run starts cleanly.
    let cfg = fresh_default_config();
    if dry_run {
        println!(
            "dry-run: would create scaffold.toml at {} (schema v{})",
            scaffold_path.display(),
            SCAFFOLD_TOML_SCHEMA_VERSION,
        );
        println!(
            "dry-run: would create {}/.scaffold/state and {}/.scaffold/logs",
            target.display(),
            target.display(),
        );
        println!(
            "dry-run: would append `.scaffold` to {}",
            target.join(".gitignore").display(),
        );
        return Ok(());
    }
    create_scaffold_dirs(target)?;
    write_text_atomic(&scaffold_path, &serialize_config(&cfg)?)?;
    ensure_scaffold_in_gitignore(target)?;
    apply_skills(target)?;

    println!(
        "scaffold.toml created at {}. Run '{bin_name} setup' to clone LEZ and build dependencies.",
        scaffold_path.display()
    );
    println!("AI skills installed under .claude/skills/, .cursor/rules/, and AGENTS.md.");
    println!(
        "If this project is building modules for basecamp, run '{bin_name} basecamp setup' to pin + build basecamp + lgpm and seed alice/bob profiles."
    );

    Ok(())
}

fn create_scaffold_dirs(target: &Path) -> DynResult<()> {
    fs::create_dir_all(target.join(".scaffold/state"))
        .with_context(|| format!("creating {}/.scaffold/state", target.display()))?;
    fs::create_dir_all(target.join(".scaffold/logs"))
        .with_context(|| format!("creating {}/.scaffold/logs", target.display()))?;
    Ok(())
}

fn fresh_default_config() -> Config {
    Config {
        version: SCAFFOLD_TOML_SCHEMA_VERSION.to_string(),
        cache_root: String::new(),
        lez: default_lez_repo(DEFAULT_LEZ.sha),
        spel: default_spel_repo(DEFAULT_SPEL.sha),
        basecamp_repo: Some(default_basecamp_repo(DEFAULT_BASECAMP_PIN)),
        lgpm_repo: Some(default_lgpm_repo(DEFAULT_LGPM_PIN)),
        wallet_home_dir: ".scaffold/wallet".to_string(),
        framework: FrameworkConfig {
            kind: FRAMEWORK_KIND_DEFAULT.to_string(),
            version: DEFAULT_FRAMEWORK_VERSION.to_string(),
            idl: FrameworkIdlConfig {
                spec: DEFAULT_FRAMEWORK_IDL_SPEC.to_string(),
                path: DEFAULT_FRAMEWORK_IDL_PATH.to_string(),
            },
        },
        localnet: LocalnetConfig::default(),
        modules: std::collections::BTreeMap::new(),
        basecamp: None,
        run: RunConfig::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::parse_config;
    use crate::constants::{BASECAMP_ATTR, LEZ_SOURCE, LGPM_ATTR, SPEL_SOURCE};
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn init_writes_parseable_v0_2_0_scaffold_toml() {
        let temp = tempdir().expect("tempdir");
        let target = temp.path();
        cmd_init_at(target, "lgs", false, false).expect("init");

        let text = fs::read_to_string(target.join("scaffold.toml")).expect("read scaffold.toml");
        let cfg = parse_config(&text).expect("parse scaffold.toml");

        assert_eq!(cfg.version, SCAFFOLD_TOML_SCHEMA_VERSION);
        assert_eq!(cfg.lez.pin, DEFAULT_LEZ.sha);
        assert_eq!(cfg.spel.pin, DEFAULT_SPEL.sha);
        assert_eq!(cfg.framework.kind, FRAMEWORK_KIND_DEFAULT);
        assert_eq!(cfg.wallet_home_dir, ".scaffold/wallet");
        assert_eq!(cfg.localnet.port, 3040);
        assert!(cfg.localnet.risc0_dev_mode);
        let bc = cfg.basecamp_repo.expect("basecamp present");
        assert_eq!(bc.attr, BASECAMP_ATTR);
        assert_eq!(bc.build, crate::model::RepoBuild::NixFlake);
        let lgpm = cfg.lgpm_repo.expect("lgpm present");
        assert_eq!(lgpm.attr, LGPM_ATTR);
    }

    #[test]
    fn init_does_not_write_url_field() {
        let temp = tempdir().expect("tempdir");
        let target = temp.path();
        cmd_init_at(target, "lgs", false, false).expect("init");
        let text = fs::read_to_string(target.join("scaffold.toml")).expect("read");
        assert!(
            !text.contains("url ="),
            "v0.2.0 scaffold.toml must not contain url field; got:\n{text}"
        );
    }

    #[test]
    fn init_does_not_persist_cache_root() {
        let temp = tempdir().expect("tempdir");
        let target = temp.path();
        cmd_init_at(target, "lgs", false, false).expect("init");
        let text = fs::read_to_string(target.join("scaffold.toml")).expect("read");
        let has_active = text
            .lines()
            .any(|l| !l.trim_start().starts_with('#') && l.contains("cache_root"));
        assert!(
            !has_active,
            "scaffold.toml should not pin cache_root by default; got:\n{text}"
        );
    }

    #[test]
    fn init_is_idempotent_when_already_at_v0_2_0_and_refreshes_skills() {
        let temp = tempdir().expect("tempdir");
        let target = temp.path();
        cmd_init_at(target, "lgs", false, false).expect("init");

        // Re-running init on an already-migrated project must succeed; it is
        // the user-facing entry point for refreshing AI skill files alongside
        // any pending schema migration.
        cmd_init_at(target, "lgs", false, false).expect("re-init must succeed and refresh skills");
        assert!(
            target.join(".claude/skills/lgs-cli/SKILL.md").is_file(),
            "claude skill must be present after re-init"
        );
        assert!(
            target.join(".cursor/rules/lgs-cli.mdc").is_file(),
            "cursor rule must be present after re-init"
        );
        assert!(
            target.join("AGENTS.md").is_file(),
            "AGENTS.md must be present after re-init"
        );
    }

    #[test]
    fn init_writes_skills_on_fresh_project() {
        let temp = tempdir().expect("tempdir");
        let target = temp.path();
        cmd_init_at(target, "lgs", false, false).expect("init");

        for name in ["lgs-cli", "lez-template", "spel-template", "basecamp"] {
            assert!(
                target
                    .join(format!(".claude/skills/{name}/SKILL.md"))
                    .is_file(),
                "missing claude skill: {name}"
            );
            assert!(
                target.join(format!(".cursor/rules/{name}.mdc")).is_file(),
                "missing cursor rule: {name}"
            );
        }
        assert!(target.join("AGENTS.md").is_file());
    }

    #[test]
    fn migrates_pre_spel_scaffold_toml() {
        let temp = tempdir().expect("tempdir");
        let target = temp.path();
        // Pre-spel: no [repos.spel], legacy [repos.lez] with url field.
        let seed = r#"# user comment
[scaffold]
version = "0.1.0"

[repos.lez]
url = "https://example.com/lez.git"
source = "https://example.com/lez.git"
path = ""
pin = "abc"

[wallet]
home_dir = ".scaffold/wallet"
"#;
        fs::write(target.join("scaffold.toml"), seed).expect("seed");

        cmd_init_at(target, "lgs", false, false).expect("migrate");

        let after = fs::read_to_string(target.join("scaffold.toml")).unwrap();
        assert!(
            after.contains("# user comment"),
            "comments preserved; got:\n{after}"
        );
        assert!(
            after.contains("version = \"0.2.0\""),
            "version bumped; got:\n{after}"
        );
        assert!(
            after.contains("[repos.spel]"),
            "spel appended; got:\n{after}"
        );
        assert!(!after.contains("url ="), "url stripped; got:\n{after}");
        // Re-parse must succeed.
        parse_config(&after).expect("re-parse migrated config");
    }

    #[test]
    fn migrates_basecamp_pin_and_modules() {
        let temp = tempdir().expect("tempdir");
        let target = temp.path();
        let seed = r#"[scaffold]
version = "0.1.1"

[repos.lez]
source = "u"
pin = "abc"

[repos.spel]
source = "v"
pin = "def"

[basecamp]
pin = "deadbeef"
source = "https://github.com/logos-co/logos-basecamp"
lgpm_flake = "github:logos-co/logos-package-manager/cafef00dcafef00dcafef00dcafef00dcafef00d#cli"
port_base = 60000
port_stride = 10

[basecamp.modules.foo]
flake = "path:./foo"
role = "project"

[basecamp.modules.bar]
flake = "github:owner/bar/abc#lgx"
role = "dependency"

[wallet]
home_dir = ".scaffold/wallet"
"#;
        fs::write(target.join("scaffold.toml"), seed).expect("seed");

        cmd_init_at(target, "lgs", false, false).expect("migrate");
        let after = fs::read_to_string(target.join("scaffold.toml")).unwrap();

        // Re-parse must succeed and surface the new shape.
        let cfg = parse_config(&after).expect("re-parse migrated config");
        let bc = cfg.basecamp_repo.expect("basecamp present");
        assert_eq!(bc.pin, "deadbeef");
        assert_eq!(bc.attr, BASECAMP_ATTR);
        let lgpm = cfg.lgpm_repo.expect("lgpm present");
        assert_eq!(lgpm.pin, "cafef00dcafef00dcafef00dcafef00dcafef00d");
        assert_eq!(lgpm.attr, "cli");
        assert_eq!(cfg.modules.len(), 2);
        assert!(cfg.modules.contains_key("foo"));
        assert!(cfg.modules.contains_key("bar"));
        // [basecamp] runtime config preserved.
        let runtime = cfg.basecamp.expect("basecamp runtime present");
        assert_eq!(runtime.port_base, 60000);
        assert_eq!(runtime.port_stride, 10);
        // Old keys gone.
        assert!(
            !after.contains("lgpm_flake"),
            "lgpm_flake removed; got:\n{after}"
        );
        assert!(
            !after.contains("[basecamp.modules"),
            "basecamp.modules removed; got:\n{after}"
        );
    }

    #[test]
    fn migration_handles_unparseable_lgpm_flake() {
        let temp = tempdir().expect("tempdir");
        let target = temp.path();
        let seed = r#"[scaffold]
version = "0.1.1"

[repos.lez]
source = "u"
pin = "abc"

[repos.spel]
source = "v"
pin = "def"

[basecamp]
pin = "deadbeef"
source = "https://example.com/basecamp"
lgpm_flake = "not-a-flake-ref"

[wallet]
home_dir = ".scaffold/wallet"
"#;
        fs::write(target.join("scaffold.toml"), seed).expect("seed");
        cmd_init_at(target, "lgs", false, false).expect("migrate");

        let after = fs::read_to_string(target.join("scaffold.toml")).unwrap();
        let cfg = parse_config(&after).expect("re-parse");
        let lgpm = cfg.lgpm_repo.expect("lgpm present");
        // Default pin written in place.
        assert_eq!(lgpm.pin, DEFAULT_LGPM_PIN);
    }

    #[test]
    fn migrates_repos_lssa_to_repos_lez() {
        let temp = tempdir().expect("tempdir");
        let target = temp.path();
        let seed = format!(
            r#"# preserved comment
[scaffold]
version = "0.1.0"

[repos.lssa]
source = "{}"
pin = "{}"

[repos.spel]
source = "{}"
pin = "{}"

[wallet]
home_dir = ".scaffold/wallet"
"#,
            LEZ_SOURCE, DEFAULT_LEZ.sha, SPEL_SOURCE, DEFAULT_SPEL.sha,
        );
        fs::write(target.join("scaffold.toml"), seed).expect("seed");
        cmd_init_at(target, "lgs", false, false).expect("migrate");

        let after = fs::read_to_string(target.join("scaffold.toml")).unwrap();
        assert!(after.contains("# preserved comment"), "{after}");
        assert!(after.contains("[repos.lez]"), "{after}");
        assert!(!after.contains("[repos.lssa]"), "{after}");
        let cfg = parse_config(&after).expect("re-parse");
        assert_eq!(cfg.lez.source, LEZ_SOURCE);
        assert_eq!(cfg.lez.pin, DEFAULT_LEZ.sha);
    }

    #[test]
    fn migration_drops_stale_lssa_when_lez_present() {
        let temp = tempdir().expect("tempdir");
        let target = temp.path();
        let seed = format!(
            r#"[scaffold]
version = "0.1.0"

[repos.lez]
source = "{}"
pin = "{}"

[repos.lssa]
source = "stale"
pin = "deadbeef"

[repos.spel]
source = "{}"
pin = "{}"

[wallet]
home_dir = ".scaffold/wallet"
"#,
            LEZ_SOURCE, DEFAULT_LEZ.sha, SPEL_SOURCE, DEFAULT_SPEL.sha,
        );
        fs::write(target.join("scaffold.toml"), seed).expect("seed");
        cmd_init_at(target, "lgs", false, false).expect("migrate");

        let after = fs::read_to_string(target.join("scaffold.toml")).unwrap();
        assert!(!after.contains("[repos.lssa]"), "{after}");
        assert!(!after.contains("stale"), "{after}");
        let cfg = parse_config(&after).expect("re-parse");
        assert_eq!(cfg.lez.source, LEZ_SOURCE);
        assert_eq!(cfg.lez.pin, DEFAULT_LEZ.sha);
    }

    #[test]
    fn migration_strips_url_only_when_no_other_changes_needed() {
        let temp = tempdir().expect("tempdir");
        let target = temp.path();
        let seed = format!(
            r#"[scaffold]
version = "0.1.1"

[repos.lez]
url = "{}"
source = "{}"
pin = "{}"

[repos.spel]
source = "{}"
pin = "{}"

[wallet]
home_dir = ".scaffold/wallet"
"#,
            LEZ_SOURCE, LEZ_SOURCE, DEFAULT_LEZ.sha, SPEL_SOURCE, DEFAULT_SPEL.sha,
        );
        fs::write(target.join("scaffold.toml"), seed).expect("seed");
        cmd_init_at(target, "lgs", false, false).expect("migrate");
        let after = fs::read_to_string(target.join("scaffold.toml")).unwrap();
        assert!(!after.contains("url ="), "url stripped; got:\n{after}");
        parse_config(&after).expect("re-parse");
    }

    #[test]
    fn init_creates_scaffold_state_and_logs_dirs() {
        let temp = tempdir().expect("tempdir");
        let target = temp.path();
        cmd_init_at(target, "lgs", false, false).expect("init");

        assert!(target.join(".scaffold/state").is_dir());
        assert!(target.join(".scaffold/logs").is_dir());
    }

    #[test]
    fn init_gitignore_is_idempotent_with_existing_scaffold_line() {
        let temp = tempdir().expect("tempdir");
        let target = temp.path();
        fs::write(target.join(".gitignore"), "target\n.scaffold\n").expect("seed");

        cmd_init_at(target, "lgs", false, false).expect("init");

        let text = fs::read_to_string(target.join(".gitignore")).unwrap();
        let count = text.lines().filter(|l| l.trim() == ".scaffold").count();
        assert_eq!(count, 1);
    }

    #[test]
    fn init_dry_run_does_not_create_scaffold_toml() {
        // C5: dry-run must be a pure preview — no scaffold.toml, no
        // .scaffold/ directories, no .gitignore mutation. Agents can call
        // dry-run as a safe "would this work?" probe before committing.
        let temp = tempdir().expect("tempdir");
        let target = temp.path();
        cmd_init_at(target, "lgs", true, false).expect("dry-run init");

        assert!(
            !target.join("scaffold.toml").exists(),
            "dry-run must not create scaffold.toml"
        );
        assert!(
            !target.join(".scaffold").exists(),
            "dry-run must not create .scaffold/"
        );
    }

    #[test]
    fn init_leaves_no_scaffold_toml_when_dir_creation_fails() {
        let temp = tempdir().expect("tempdir");
        let target = temp.path();
        // Seed a regular file at `.scaffold` so `create_dir_all(".scaffold/state")` fails.
        // Dir creation is the first filesystem mutation in fresh-init; if it fails the
        // user must be left with no scaffold.toml so a retry starts from a clean state.
        fs::write(target.join(".scaffold"), b"not a dir").expect("seed");

        let err = cmd_init_at(target, "lgs", false, false).expect_err("dir creation should fail");
        assert!(
            err.to_string().contains(".scaffold"),
            "error mentions .scaffold path: {err}"
        );
        assert!(
            !target.join("scaffold.toml").exists(),
            "scaffold.toml must not be written when dir creation fails",
        );
    }

    #[test]
    fn init_migration_writes_backup_by_default() {
        // C5: a migration mutates scaffold.toml in place. By default `init`
        // now writes scaffold.toml.bak alongside so a botched migration can
        // be reverted by hand.
        let temp = tempdir().expect("tempdir");
        let target = temp.path();
        let original = r#"[scaffold]
version = "0.1.0"

[repos.lez]
source = "https://example.com/lez.git"
pin = "abc"

[wallet]
home_dir = ".scaffold/wallet"
"#;
        fs::write(target.join("scaffold.toml"), original).expect("seed");

        cmd_init_at(target, "lgs", false, false).expect("migrate");

        let backup = target.join("scaffold.toml.bak");
        assert!(backup.exists(), "backup must exist at {}", backup.display());
        let backup_text = fs::read_to_string(&backup).expect("read backup");
        assert_eq!(
            backup_text, original,
            "backup must preserve the pre-migration scaffold.toml verbatim"
        );

        let after = fs::read_to_string(target.join("scaffold.toml")).expect("read");
        assert!(
            after.contains("0.2.0"),
            "scaffold.toml migrated; got:\n{after}"
        );
    }

    #[test]
    fn init_migration_skips_backup_with_no_backup_flag() {
        // C5: opt-out for users who manage backups via VCS or want the
        // smallest possible filesystem footprint.
        let temp = tempdir().expect("tempdir");
        let target = temp.path();
        let original = r#"[scaffold]
version = "0.1.0"

[repos.lez]
source = "https://example.com/lez.git"
pin = "abc"

[wallet]
home_dir = ".scaffold/wallet"
"#;
        fs::write(target.join("scaffold.toml"), original).expect("seed");

        cmd_init_at(target, "lgs", false, true).expect("migrate");

        assert!(
            !target.join("scaffold.toml.bak").exists(),
            "no-backup flag must skip writing scaffold.toml.bak"
        );
    }

    #[test]
    fn init_migration_refuses_to_overwrite_existing_backup() {
        // PR #86 review: `fs::copy` to scaffold.toml.bak would silently clobber
        // an existing backup (e.g. from a prior migration the user kept around,
        // or a hand-curated snapshot). Refuse instead and surface both ways out.
        let temp = tempdir().expect("tempdir");
        let target = temp.path();
        let original = r#"[scaffold]
version = "0.1.0"

[repos.lez]
source = "https://example.com/lez.git"
pin = "abc"

[wallet]
home_dir = ".scaffold/wallet"
"#;
        fs::write(target.join("scaffold.toml"), original).expect("seed");
        // Pre-existing .bak that must NOT be clobbered.
        let prior_backup = b"prior backup content the user wants to keep";
        fs::write(target.join("scaffold.toml.bak"), prior_backup).expect("seed bak");

        let err = cmd_init_at(target, "lgs", false, false).expect_err("should refuse");
        let msg = format!("{err:#}");
        assert!(msg.contains("scaffold.toml.bak"), "{msg}");
        assert!(msg.contains("--no-backup"), "{msg}");

        let after_bak = fs::read(target.join("scaffold.toml.bak")).expect("read bak");
        assert_eq!(
            after_bak, prior_backup,
            "refusal must leave the existing backup untouched"
        );
        let after = fs::read_to_string(target.join("scaffold.toml")).expect("read");
        assert_eq!(
            after, original,
            "refusal must leave scaffold.toml unmigrated"
        );

        // The escape hatch keeps the migration moving without writing a backup.
        cmd_init_at(target, "lgs", false, true).expect("migrate with --no-backup");
        let after = fs::read_to_string(target.join("scaffold.toml")).expect("read");
        assert!(
            after.contains("0.2.0"),
            "no-backup form must still migrate; got:\n{after}"
        );
    }

    #[test]
    fn init_dry_run_on_pre_migration_scaffold_toml_does_not_mutate() {
        // C5: dry-run on a migration candidate must leave scaffold.toml
        // untouched and not create the .bak side file either.
        let temp = tempdir().expect("tempdir");
        let target = temp.path();
        let original = r#"[scaffold]
version = "0.1.0"

[repos.lez]
source = "https://example.com/lez.git"
pin = "abc"

[wallet]
home_dir = ".scaffold/wallet"
"#;
        fs::write(target.join("scaffold.toml"), original).expect("seed");

        cmd_init_at(target, "lgs", true, false).expect("dry-run migrate");

        let after = fs::read_to_string(target.join("scaffold.toml")).expect("read");
        assert_eq!(
            after, original,
            "dry-run on migration must not mutate scaffold.toml"
        );
        assert!(
            !target.join("scaffold.toml.bak").exists(),
            "dry-run must not write scaffold.toml.bak"
        );
    }

    #[test]
    fn init_completes_wedged_state_when_dirs_missing() {
        let temp = tempdir().expect("tempdir");
        let target = temp.path();
        // First, a normal init.
        cmd_init_at(target, "lgs", false, false).expect("init");
        // Simulate a wedge: scaffold.toml landed at v0.2.0 but `.scaffold/` is gone.
        fs::remove_dir_all(target.join(".scaffold")).expect("nuke .scaffold");
        assert!(target.join("scaffold.toml").exists());
        assert!(!target.join(".scaffold/state").exists());

        // Re-running init must complete the partial install, not refuse.
        cmd_init_at(target, "lgs", false, false).expect("recover from wedge");
        assert!(target.join(".scaffold/state").is_dir());
        assert!(target.join(".scaffold/logs").is_dir());
    }

    #[test]
    fn init_rerun_succeeds_when_already_initialized_and_dirs_present() {
        let temp = tempdir().expect("tempdir");
        let target = temp.path();
        cmd_init_at(target, "lgs", false, false).expect("init");
        cmd_init_at(target, "lgs", false, false).expect("second init should refresh skills");
        assert!(target.join(".scaffold/state").is_dir());
        assert!(target.join(".scaffold/logs").is_dir());
        assert!(target.join(".claude/skills/lgs-cli/SKILL.md").is_file());
    }
}
