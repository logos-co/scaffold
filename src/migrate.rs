//! Pre-0.2.0 → v0.2.0 in-place migration of `scaffold.toml`.
//!
//! `commands/init.rs` is the only caller. The migrator mutates a parsed
//! `toml_edit::DocumentMut` so user comments, key ordering, and unrelated
//! sections survive. Anything the parser rejects in `config::detect_old_schema`
//! is exactly what this module knows how to rewrite.

use anyhow::{anyhow, bail};
use toml_edit::{value, DocumentMut, Item, Table};

use crate::constants::{
    BASECAMP_ATTR, BASECAMP_SOURCE, DEFAULT_BASECAMP_PIN, DEFAULT_LGPM_PIN, DEFAULT_SPEL,
    LGPM_ATTR, LGPM_SOURCE, SCAFFOLD_TOML_SCHEMA_VERSION,
};
use crate::model::{RepoBuild, RepoRef};
use crate::DynResult;

#[derive(Default)]
pub(crate) struct MigrationReport {
    pub(crate) changes: Vec<String>,
    /// Set when migration succeeded but a field was unparseable and the user
    /// must hand-edit. Currently only triggered by malformed `lgpm_flake`.
    pub(crate) hand_edit_hint: Option<String>,
}

/// Mutate `doc` in place from any pre-0.2.0 schema to v0.2.0. Preserves
/// comments, key ordering, and unrelated sections via toml_edit. Returns a
/// report listing what changed; an empty report means "already migrated."
///
/// The input may be:
/// - A pre-spel scaffold.toml (no `[repos.spel]` section). The original
///   migration's job — append the section.
/// - A 0.1.x-era scaffold.toml with `url` fields, `[basecamp].pin/.source/
///   .lgpm_flake`, or `[basecamp.modules.*]`. Reshape all of those.
/// - A mix of the above.
pub(crate) fn migrate_to_v0_2_0(doc: &mut DocumentMut) -> DynResult<MigrationReport> {
    let mut report = MigrationReport::default();

    // Short-circuit when the file is already at the current schema version.
    // The migrator's rewrites (lssa→lez drop, url strip, basecamp reshape, etc.)
    // are pre-v0.2.0 fixups; running them against an already-current file can
    // silently rewrite intentionally non-conformant content (e.g. a hand-kept
    // `[repos.lssa]` for a fork).
    if doc
        .get("scaffold")
        .and_then(Item::as_table)
        .and_then(|t| t.get("version"))
        .and_then(Item::as_str)
        == Some(SCAFFOLD_TOML_SCHEMA_VERSION)
    {
        return Ok(report);
    }

    // Ensure [scaffold] exists; bump version.
    let scaffold = doc.entry("scaffold").or_insert(Item::Table({
        let mut t = Table::new();
        t.set_implicit(false);
        t
    }));
    let scaffold_table = scaffold
        .as_table_mut()
        .ok_or_else(|| anyhow!("[scaffold] is not a table"))?;
    let current_version = scaffold_table
        .get("version")
        .and_then(Item::as_str)
        .unwrap_or("")
        .to_string();
    if current_version != SCAFFOLD_TOML_SCHEMA_VERSION {
        scaffold_table["version"] = value(SCAFFOLD_TOML_SCHEMA_VERSION);
        report.changes.push(format!(
            "bumped [scaffold].version: {:?} -> {:?}",
            if current_version.is_empty() {
                "<unset>"
            } else {
                current_version.as_str()
            },
            SCAFFOLD_TOML_SCHEMA_VERSION,
        ));
    }

    // [repos.lssa] -> [repos.lez] alias rename. If both sections exist, the
    // stale `lssa` is dropped (lez wins) — config::detect_old_schema rejects
    // any lssa section, so leaving it behind would keep the file unparseable.
    if let Some(repos) = doc.get_mut("repos").and_then(Item::as_table_mut) {
        if repos.contains_key("lssa") {
            if let Some(lssa) = repos.remove("lssa") {
                if repos.contains_key("lez") {
                    report
                        .changes
                        .push("dropped stale [repos.lssa] (kept [repos.lez])".to_string());
                } else {
                    repos.insert("lez", lssa);
                    report
                        .changes
                        .push("renamed [repos.lssa] -> [repos.lez]".to_string());
                }
            }
        }
    }

    // Drop `url` from [repos.lez] / [repos.spel].
    for name in ["lez", "spel"] {
        if let Some(repo) = doc
            .get_mut("repos")
            .and_then(Item::as_table_mut)
            .and_then(|r| r.get_mut(name).and_then(Item::as_table_mut))
        {
            if repo.remove("url").is_some() {
                report
                    .changes
                    .push(format!("removed [repos.{name}].url (use `source` only)"));
            }
        }
    }

    // Append [repos.spel] if missing (pre-spel migration semantics).
    let spel_missing = doc
        .get("repos")
        .and_then(Item::as_table)
        .and_then(|r| r.get("spel"))
        .is_none();
    if spel_missing {
        // Vendor-detection: if existing [repos.lez].path is `.scaffold/repos/lez`,
        // mirror the layout for spel. Otherwise leave path empty (portable).
        let lez_path = doc
            .get("repos")
            .and_then(Item::as_table)
            .and_then(|r| r.get("lez").and_then(Item::as_table))
            .and_then(|t| t.get("path").and_then(Item::as_str))
            .unwrap_or("")
            .to_string();
        let mut spel = crate::config::default_spel_repo(DEFAULT_SPEL.sha);
        if lez_path == ".scaffold/repos/lez" {
            spel.path = ".scaffold/repos/spel".to_string();
        }
        write_repo_ref_via_toml_edit(doc, "spel", &spel);
        report
            .changes
            .push("appended [repos.spel] with default pin".to_string());
    }

    // Migrate [basecamp].pin / .source -> [repos.basecamp].
    let mut basecamp_pin = None;
    let mut basecamp_source = None;
    let mut lgpm_flake = None;
    if let Some(bc) = doc.get_mut("basecamp").and_then(Item::as_table_mut) {
        if let Some(s) = bc.get("pin").and_then(Item::as_str) {
            basecamp_pin = Some(s.to_string());
        }
        if let Some(s) = bc.get("source").and_then(Item::as_str) {
            basecamp_source = Some(s.to_string());
        }
        if let Some(s) = bc.get("lgpm_flake").and_then(Item::as_str) {
            lgpm_flake = Some(s.to_string());
        }
    }

    let need_basecamp_repo = basecamp_pin.is_some() || basecamp_source.is_some();
    if need_basecamp_repo {
        let pin = basecamp_pin
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| DEFAULT_BASECAMP_PIN.to_string());
        let source = basecamp_source
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| BASECAMP_SOURCE.to_string());
        let mut repo = crate::config::default_basecamp_repo(&pin);
        repo.source = source;
        repo.attr = BASECAMP_ATTR.to_string();
        write_repo_ref_via_toml_edit(doc, "basecamp", &repo);
        report
            .changes
            .push("migrated [basecamp].pin / .source -> [repos.basecamp]".to_string());
    }

    // Migrate [basecamp].lgpm_flake -> [repos.lgpm].
    if let Some(flake_ref) = lgpm_flake {
        if !flake_ref.is_empty() {
            match split_flake_ref(&flake_ref) {
                Some((source, pin, attr)) => {
                    let mut repo = crate::config::default_lgpm_repo(&pin);
                    repo.source = source;
                    repo.attr = if attr.is_empty() {
                        LGPM_ATTR.to_string()
                    } else {
                        attr
                    };
                    write_repo_ref_via_toml_edit(doc, "lgpm", &repo);
                    report
                        .changes
                        .push("migrated [basecamp].lgpm_flake -> [repos.lgpm]".to_string());
                }
                None => {
                    // Unparseable — write a placeholder repo with default
                    // pin and tell the user to fix it by hand. We still
                    // strip the old key so the file ends up valid.
                    let repo = crate::config::default_lgpm_repo(DEFAULT_LGPM_PIN);
                    write_repo_ref_via_toml_edit(doc, "lgpm", &repo);
                    report.hand_edit_hint = Some(format!(
                        "could not parse [basecamp].lgpm_flake = {flake_ref:?}; wrote default \
                         [repos.lgpm] (source={LGPM_SOURCE}, pin={DEFAULT_LGPM_PIN}). Edit \
                         scaffold.toml to set the right pin."
                    ));
                    report
                        .changes
                        .push("migrated [basecamp].lgpm_flake -> [repos.lgpm] (default pin; verify by hand)".to_string());
                }
            }
        }
    }

    // [basecamp.modules.*] -> [modules.*]
    let mut moved_modules = Vec::new();
    if let Some(bc) = doc.get_mut("basecamp").and_then(Item::as_table_mut) {
        if let Some(modules_item) = bc.get("modules") {
            if let Some(modules_table) = modules_item.as_table() {
                for (name, item) in modules_table.iter() {
                    if let Some(t) = item.as_table() {
                        moved_modules.push((name.to_string(), t.clone()));
                    }
                }
            }
        }
        bc.remove("modules");
    }
    if !moved_modules.is_empty() {
        let modules_root = doc.entry("modules").or_insert(Item::Table({
            let mut t = Table::new();
            t.set_implicit(true);
            t
        }));
        let modules_table = modules_root
            .as_table_mut()
            .ok_or_else(|| anyhow!("[modules] is not a table"))?;
        for (name, t) in &moved_modules {
            if modules_table.contains_key(name) {
                bail!(
                    "module name collision during migration: `{name}` exists in both \
                     [basecamp.modules.{name}] and [modules.{name}]. Resolve by renaming \
                     or removing one entry in scaffold.toml before re-running init."
                );
            }
            modules_table.insert(name, Item::Table(t.clone()));
        }
        report.changes.push(format!(
            "moved [basecamp.modules.*] -> [modules.*] ({} entr{})",
            moved_modules.len(),
            if moved_modules.len() == 1 { "y" } else { "ies" },
        ));
    }

    // Strip migrated keys from [basecamp].
    if let Some(bc) = doc.get_mut("basecamp").and_then(Item::as_table_mut) {
        for stale in ["pin", "source", "lgpm_flake"] {
            bc.remove(stale);
        }
        // If [basecamp] is now empty (no port_base/port_stride either),
        // drop the section entirely.
        if bc.iter().next().is_none() {
            doc.as_table_mut().remove("basecamp");
        }
    }

    Ok(report)
}

fn write_repo_ref_via_toml_edit(doc: &mut DocumentMut, name: &str, repo: &RepoRef) {
    let repos = doc.entry("repos").or_insert(Item::Table({
        let mut t = Table::new();
        t.set_implicit(true);
        t
    }));
    let repos_table = repos.as_table_mut().expect("repos is a table");
    repos_table.set_implicit(true);
    let table = repos_table
        .entry(name)
        .or_insert(Item::Table(Table::new()))
        .as_table_mut()
        .expect("repo is a table");
    table["source"] = value(&repo.source);
    table["pin"] = value(&repo.pin);
    if repo.build != RepoBuild::default() {
        table["build"] = value(repo.build.as_str());
    } else {
        table.remove("build");
    }
    if !repo.attr.is_empty() {
        table["attr"] = value(&repo.attr);
    } else {
        table.remove("attr");
    }
    if !repo.path.is_empty() {
        table["path"] = value(&repo.path);
    } else {
        table.remove("path");
    }
}

/// Parse a flake-style ref like `github:owner/repo/<sha>#attr` into its
/// `(source, pin, attr)` components for migration of pre-0.2.0
/// `[basecamp].lgpm_flake` strings. Returns `None` if the ref doesn't
/// match the expected shape; the caller surfaces that as a hand-edit hint.
fn split_flake_ref(flake_ref: &str) -> Option<(String, String, String)> {
    let (without_attr, attr) = match flake_ref.split_once('#') {
        Some((head, tail)) if !tail.is_empty() => (head, tail.to_string()),
        _ => return None,
    };
    // Expect `<scheme>:<owner>/<repo>/<sha>` or `<scheme>:<owner>/<repo>` (no
    // pin — caller decides). We split off the trailing path segment as the
    // pin if it looks SHA-shaped; otherwise the whole thing is the source
    // and the caller has to fill in the pin separately.
    let (scheme, rest) = without_attr.split_once(':')?;
    let segments: Vec<&str> = rest.split('/').collect();
    if segments.len() < 2 {
        return None;
    }
    let last = segments[segments.len() - 1];
    if last.len() == 40 && last.chars().all(|c| c.is_ascii_hexdigit()) {
        let source = format!("{scheme}:{}", segments[..segments.len() - 1].join("/"));
        Some((source, last.to_string(), attr))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrator_short_circuits_on_current_version_and_preserves_stray_lssa() {
        // A user keeps a hand-rolled [repos.lssa] in a v0.2.0 file (e.g. a fork
        // that re-uses the old name). The migrator must not silently drop it.
        let seed = r#"[scaffold]
version = "0.2.0"

[repos.lssa]
source = "https://example.com/lssa.git"
pin = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef"

[repos.lez]
source = "https://example.com/lez.git"
pin = "abc123abc123abc123abc123abc123abc123abc1"

[repos.spel]
source = "https://example.com/spel.git"
pin = "feedfacefeedfacefeedfacefeedfacefeedface"
"#;
        let mut doc: DocumentMut = seed.parse().expect("parse seed");
        let report = migrate_to_v0_2_0(&mut doc).expect("migrate");

        assert!(
            report.changes.is_empty(),
            "no changes expected when already at v0.2.0, got: {:?}",
            report.changes,
        );
        let after = doc.to_string();
        assert!(
            after.contains("[repos.lssa]"),
            "stray [repos.lssa] must be preserved; got:\n{after}"
        );
        assert_eq!(after, seed, "document unchanged when version is current");
    }

    #[test]
    fn split_flake_ref_pulls_apart_canonical_lgpm_form() {
        let parsed = split_flake_ref(
            "github:logos-co/logos-package-manager/e5c25989861f4487c3dc8c7b3bc0062bcbc3221f#cli",
        )
        .expect("split");
        assert_eq!(parsed.0, "github:logos-co/logos-package-manager");
        assert_eq!(parsed.1, "e5c25989861f4487c3dc8c7b3bc0062bcbc3221f");
        assert_eq!(parsed.2, "cli");
    }

    #[test]
    fn split_flake_ref_returns_none_when_no_pin_in_path() {
        // No SHA in the path — caller must fill in the pin by hand.
        assert!(split_flake_ref("github:logos-co/logos-package-manager#cli").is_none());
    }

    #[test]
    fn split_flake_ref_returns_none_when_no_attr() {
        assert!(split_flake_ref(
            "github:logos-co/logos-package-manager/e5c25989861f4487c3dc8c7b3bc0062bcbc3221f"
        )
        .is_none());
    }

    #[test]
    fn migration_bails_on_module_name_collision_between_basecamp_modules_and_modules() {
        // Pre-0.2.0 file that has BOTH a legacy [basecamp.modules.foo] and a
        // pre-existing top-level [modules.foo] (hand-edited or half-migrated).
        // The migrator must refuse rather than silently overwrite the existing
        // [modules.foo] table — that is silent data loss in a version-controlled
        // config file. The error must name the colliding module.
        let input = r#"[scaffold]
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

[basecamp.modules.foo]
flake = "path:./legacy-foo"
role = "project"

[modules.foo]
flake = "path:./new-foo"
role = "project"
"#;
        let mut doc: DocumentMut = input.parse().expect("parse seed");
        let err = match migrate_to_v0_2_0(&mut doc) {
            Ok(_) => panic!("expected collision error, migration succeeded"),
            Err(e) => e,
        };
        let msg = format!("{err:#}");
        assert!(
            msg.contains("module name collision"),
            "error should name the failure mode; got: {msg}"
        );
        assert!(
            msg.contains("foo"),
            "error should name the colliding module; got: {msg}"
        );

        // The pre-existing [modules.foo] must still carry the hand-edited
        // flake — i.e. it was NOT clobbered before the bail.
        let preserved = doc
            .get("modules")
            .and_then(Item::as_table)
            .and_then(|m| m.get("foo"))
            .and_then(Item::as_table)
            .and_then(|t| t.get("flake"))
            .and_then(Item::as_str)
            .expect("modules.foo.flake preserved");
        assert_eq!(preserved, "path:./new-foo");
    }

    #[test]
    fn migration_moves_basecamp_modules_when_no_collision() {
        // Sanity counterpart to the collision test: without a colliding
        // [modules.<name>] entry, the modules move succeeds and the new
        // [modules.foo] carries the legacy flake.
        let input = r#"[scaffold]
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

[basecamp.modules.foo]
flake = "path:./legacy-foo"
role = "project"
"#;
        let mut doc: DocumentMut = input.parse().expect("parse seed");
        let report = migrate_to_v0_2_0(&mut doc).expect("migration succeeds");
        assert!(
            report
                .changes
                .iter()
                .any(|c| c.contains("[basecamp.modules.*] -> [modules.*]")),
            "expected modules-move entry in report; got: {:?}",
            report.changes
        );
        let moved = doc
            .get("modules")
            .and_then(Item::as_table)
            .and_then(|m| m.get("foo"))
            .and_then(Item::as_table)
            .and_then(|t| t.get("flake"))
            .and_then(Item::as_str)
            .expect("modules.foo.flake present after move");
        assert_eq!(moved, "path:./legacy-foo");
    }
}
