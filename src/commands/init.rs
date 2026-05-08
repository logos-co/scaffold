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
use crate::state::write_text;
use crate::template::project::ensure_scaffold_in_gitignore;
use crate::DynResult;

pub(crate) fn cmd_init(bin_name: &str) -> DynResult<()> {
    let cwd = env::current_dir()?;
    cmd_init_at(&cwd, bin_name)
}

pub(crate) fn cmd_init_at(target: &Path, bin_name: &str) -> DynResult<()> {
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
        if report.changes.is_empty() {
            bail!(
                "scaffold.toml at {} is already at schema v{} — nothing to migrate",
                target.display(),
                SCAFFOLD_TOML_SCHEMA_VERSION,
            );
        }
        write_text(&scaffold_path, &doc.to_string())?;
        println!(
            "scaffold.toml in {} migrated to schema v{}.",
            target.display(),
            SCAFFOLD_TOML_SCHEMA_VERSION,
        );
        for change in report.changes {
            println!("  - {change}");
        }
        if let Some(hint) = report.hand_edit_hint {
            println!("  ! {hint}");
        }
        println!("Run `{bin_name} setup` to clone and build per the new schema.");
        return Ok(());
    }

    // Fresh init — schema 0.2.0 by construction.
    let cfg = fresh_default_config();
    write_text(&scaffold_path, &serialize_config(&cfg)?)?;
    fs::create_dir_all(target.join(".scaffold/state"))
        .with_context(|| format!("creating {}/.scaffold/state", target.display()))?;
    fs::create_dir_all(target.join(".scaffold/logs"))
        .with_context(|| format!("creating {}/.scaffold/logs", target.display()))?;
    ensure_scaffold_in_gitignore(target)?;

    println!(
        "scaffold.toml created at {}. Run '{bin_name} setup' to clone LEZ and build dependencies.",
        scaffold_path.display()
    );
    println!(
        "If this project is building modules for basecamp, run '{bin_name} basecamp setup' to pin + build basecamp + lgpm and seed alice/bob profiles."
    );

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
        cmd_init_at(target, "lgs").expect("init");

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
        cmd_init_at(target, "lgs").expect("init");
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
        cmd_init_at(target, "lgs").expect("init");
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
    fn init_refuses_when_already_at_v0_2_0() {
        let temp = tempdir().expect("tempdir");
        let target = temp.path();
        cmd_init_at(target, "lgs").expect("init");
        let err = cmd_init_at(target, "lgs").expect_err("should refuse");
        assert!(err.to_string().contains("already at schema"), "{err}");
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

        cmd_init_at(target, "lgs").expect("migrate");

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

        cmd_init_at(target, "lgs").expect("migrate");
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
        cmd_init_at(target, "lgs").expect("migrate");

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
        cmd_init_at(target, "lgs").expect("migrate");

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
        cmd_init_at(target, "lgs").expect("migrate");

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
        cmd_init_at(target, "lgs").expect("migrate");
        let after = fs::read_to_string(target.join("scaffold.toml")).unwrap();
        assert!(!after.contains("url ="), "url stripped; got:\n{after}");
        parse_config(&after).expect("re-parse");
    }

    #[test]
    fn init_creates_scaffold_state_and_logs_dirs() {
        let temp = tempdir().expect("tempdir");
        let target = temp.path();
        cmd_init_at(target, "lgs").expect("init");

        assert!(target.join(".scaffold/state").is_dir());
        assert!(target.join(".scaffold/logs").is_dir());
    }

    #[test]
    fn init_gitignore_is_idempotent_with_existing_scaffold_line() {
        let temp = tempdir().expect("tempdir");
        let target = temp.path();
        fs::write(target.join(".gitignore"), "target\n.scaffold\n").expect("seed");

        cmd_init_at(target, "lgs").expect("init");

        let text = fs::read_to_string(target.join(".gitignore")).unwrap();
        let count = text.lines().filter(|l| l.trim() == ".scaffold").count();
        assert_eq!(count, 1);
    }
}
