//! Parser and serializer for `scaffold.toml`.
//!
//! Schema version 0.2.0 (see `SCAFFOLD_TOML_SCHEMA_VERSION` in `constants.rs`)
//! organizes the file into three orthogonal namespaces:
//!
//! - `[repos.<name>]` — pinned external git deps. One field shape:
//!   `source`, `pin`, optional `build` (default `"cargo"`), optional `attr`,
//!   optional `path` override. Today's `<name>`s: `lez`, `spel`,
//!   `basecamp`, `lgpm`. Adding a fifth is a one-section addition.
//! - `[modules.<name>]` — Logos modules the project ships. `flake` + `role`.
//!   `basecamp install` / `launch` / `build-portable` consume them, but
//!   they aren't basecamp's property — moved out from `[basecamp.modules.*]`
//!   in 0.2.0.
//! - `[<feature>]` — runtime config per feature: `[scaffold]`, `[wallet]`,
//!   `[framework]`, `[localnet]`, `[basecamp]` (port allocation only).
//!
//! Pre-0.2.0 configs (with `[basecamp].pin` / `.source` / `.lgpm_flake`,
//! `[basecamp.modules.*]`, or `[repos.{lez,spel}].url`) are rejected by
//! `detect_old_schema` with a targeted error pointing at `init`. The
//! corresponding rewrite lives in `crate::migrate`.

use anyhow::{anyhow, bail, Context};
use toml_edit::{value, DocumentMut, Item, Table};

use crate::constants::{
    BASECAMP_ATTR, BASECAMP_SOURCE, DEFAULT_FRAMEWORK_IDL_PATH, DEFAULT_FRAMEWORK_IDL_SPEC,
    DEFAULT_FRAMEWORK_VERSION, FRAMEWORK_KIND_DEFAULT, LEZ_SOURCE, LGPM_ATTR, LGPM_SOURCE,
    SCAFFOLD_TOML_SCHEMA_VERSION, SPEL_SOURCE,
};
use crate::model::{
    BasecampConfig, Config, FrameworkConfig, FrameworkIdlConfig, LocalnetConfig, ModuleEntry,
    ModuleRole, RepoBuild, RepoRef, RunConfig, RunProfile,
};
use crate::DynResult;

/// Parse a `scaffold.toml` text into a `Config`. Pre-0.2.0 schemas are
/// rejected with a targeted error pointing at `init`.
pub(crate) fn parse_config(text: &str) -> DynResult<Config> {
    let doc: DocumentMut = text
        .parse()
        .context("invalid scaffold.toml: TOML parse error")?;

    let scaffold = doc
        .get("scaffold")
        .and_then(Item::as_table)
        .ok_or_else(|| anyhow!("invalid scaffold.toml: missing [scaffold] section"))?;
    let version = read_string(scaffold, "version")
        .ok_or_else(|| anyhow!("invalid scaffold.toml: missing [scaffold].version"))?;

    detect_old_schema(&doc, &version)?;

    if version != SCAFFOLD_TOML_SCHEMA_VERSION {
        bail!(
            "scaffold.toml has [scaffold].version = {version:?}; this build expects {expected:?}. \
             Run `logos-scaffold init` to migrate; existing settings are preserved.",
            expected = SCAFFOLD_TOML_SCHEMA_VERSION,
        );
    }

    let cache_root = read_string(scaffold, "cache_root").unwrap_or_default();

    let lez = parse_repo_ref(&doc, "lez")?
        .ok_or_else(|| anyhow!("invalid scaffold.toml: missing [repos.lez]"))?;
    let spel = parse_repo_ref(&doc, "spel")?
        .ok_or_else(|| anyhow!("invalid scaffold.toml: missing [repos.spel]"))?;
    let basecamp_repo = parse_repo_ref(&doc, "basecamp")?;
    let lgpm_repo = parse_repo_ref(&doc, "lgpm")?;

    let modules = parse_modules(&doc)?;
    let basecamp = parse_basecamp_runtime(&doc)?;
    let run = parse_run(&doc)?;
    let framework = parse_framework(&doc);
    let localnet = parse_localnet(&doc)?;
    let wallet_home_dir = doc
        .get("wallet")
        .and_then(Item::as_table)
        .and_then(|t| read_string(t, "home_dir"))
        .unwrap_or_else(|| ".scaffold/wallet".to_string());

    Ok(Config {
        version,
        cache_root,
        lez,
        spel,
        basecamp_repo,
        lgpm_repo,
        wallet_home_dir,
        framework,
        localnet,
        modules,
        basecamp,
        run,
    })
}

fn parse_run(doc: &DocumentMut) -> DynResult<RunConfig> {
    let Some(run_table) = doc.get("run").and_then(Item::as_table) else {
        return Ok(RunConfig::default());
    };

    let default_profile = read_string(run_table, "default_profile");
    let inline_reset = run_table
        .get("reset")
        .and_then(Item::as_value)
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let inline_post_deploy = parse_post_deploy(run_table.get("post_deploy"))?;

    let mut profiles: std::collections::BTreeMap<String, RunProfile> =
        std::collections::BTreeMap::new();
    if let Some(profiles_table) = run_table.get("profiles").and_then(Item::as_table) {
        for (name, item) in profiles_table.iter() {
            let table = item.as_table().ok_or_else(|| {
                anyhow!("invalid scaffold.toml: [run.profiles.{name}] is not a table")
            })?;
            let reset = table
                .get("reset")
                .and_then(Item::as_value)
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let post_deploy = parse_post_deploy(table.get("post_deploy"))?;
            profiles.insert(name.to_string(), RunProfile { reset, post_deploy });
        }
    }

    if let Some(name) = &default_profile {
        if !profiles.contains_key(name) {
            bail!(
                "invalid scaffold.toml: [run].default_profile = {name:?} but no [run.profiles.{name}] section"
            );
        }
    }

    Ok(RunConfig {
        default_profile,
        inline: RunProfile {
            reset: inline_reset,
            post_deploy: inline_post_deploy,
        },
        profiles,
    })
}

fn parse_post_deploy(item: Option<&Item>) -> DynResult<Vec<String>> {
    let Some(item) = item else {
        return Ok(Vec::new());
    };
    if let Some(s) = item.as_str() {
        return Ok(if s.is_empty() {
            Vec::new()
        } else {
            vec![s.to_string()]
        });
    }
    if let Some(arr) = item.as_array() {
        let mut out = Vec::with_capacity(arr.len());
        for v in arr.iter() {
            let s = v.as_str().ok_or_else(|| {
                anyhow!("invalid scaffold.toml: post_deploy entries must be strings")
            })?;
            out.push(s.to_string());
        }
        return Ok(out);
    }
    bail!("invalid scaffold.toml: post_deploy must be a string or array of strings")
}

/// Reject pre-0.2.0 schemas with a targeted error naming the section that's
/// stale and `init` as the fix. Detection is pragmatic: any single old-shape
/// signal is enough.
fn detect_old_schema(doc: &DocumentMut, version: &str) -> DynResult<()> {
    let mut markers: Vec<&str> = Vec::new();

    // Old version stamp. Any other version mismatch (e.g. prerelease tags or
    // hand-edits) is caught downstream in `parse_config` with a more specific
    // "this build expects X" message; `init`'s migrator bumps the version
    // regardless of origin.
    if version != SCAFFOLD_TOML_SCHEMA_VERSION
        && (version.starts_with("0.1.") || version == "0.1" || version == "0.0")
    {
        markers.push("[scaffold].version is pre-0.2.0");
    }

    // [repos.lssa] — pre-spel-era alias for [repos.lez]. Even if no other
    // signals fire (e.g. the user hand-bumped the version stamp), the
    // canonical name has changed and `init` is responsible for the rename.
    let repos_table = doc.get("repos").and_then(Item::as_table);
    if let Some(repos) = repos_table {
        if repos.get("lssa").is_some() {
            markers.push("[repos.lssa] renamed to [repos.lez] in 0.2.0");
        }
    }

    // [repos.{lez,spel}].url — dropped in 0.2.0; source is the single field.
    // (lssa is checked above as its own signal.)
    for name in ["lez", "spel"] {
        let table = repos_table.and_then(|t| t.get(name).and_then(Item::as_table));
        if let Some(table) = table {
            if table.get("url").is_some() {
                markers.push("[repos.lez|spel].url is removed in 0.2.0 (use `source` only)");
                break;
            }
        }
    }

    // Old [basecamp] shape: pin / source / lgpm_flake at the root.
    if let Some(bc) = doc.get("basecamp").and_then(Item::as_table) {
        for stale in ["pin", "source", "lgpm_flake"] {
            if bc.get(stale).is_some() {
                markers.push("[basecamp] has pin/source/lgpm_flake (moved to [repos.basecamp] / [repos.lgpm])");
                break;
            }
        }
    }

    // [basecamp.modules.*] — moved to [modules.*].
    if let Some(bc) = doc.get("basecamp").and_then(Item::as_table) {
        if let Some(modules) = bc.get("modules").and_then(Item::as_table) {
            if modules.iter().next().is_some() {
                markers.push("[basecamp.modules.*] moved to [modules.*]");
            }
        }
    }

    if markers.is_empty() {
        return Ok(());
    }

    let detail = markers.join("; ");
    bail!(
        "scaffold.toml uses an old schema ({detail}). \
         Run `logos-scaffold init` to migrate to v{SCAFFOLD_TOML_SCHEMA_VERSION}; \
         existing settings are preserved."
    );
}

fn parse_repo_ref(doc: &DocumentMut, name: &str) -> DynResult<Option<RepoRef>> {
    // [repos.<name>] is the canonical key. Pre-spel-era configs that used
    // [repos.lssa] are rejected upstream in `detect_old_schema` so users are
    // pushed through `init` for the rename — no alias acceptance here.
    let Some(table) = doc
        .get("repos")
        .and_then(Item::as_table)
        .and_then(|t| t.get(name).and_then(Item::as_table))
    else {
        return Ok(None);
    };

    let source = read_string(table, "source")
        .ok_or_else(|| anyhow!("invalid scaffold.toml: missing [repos.{name}].source"))?;
    let pin = read_string(table, "pin")
        .ok_or_else(|| anyhow!("invalid scaffold.toml: missing [repos.{name}].pin"))?;
    let build = match read_string(table, "build") {
        Some(s) => RepoBuild::parse(&s).ok_or_else(|| {
            anyhow!("invalid scaffold.toml: [repos.{name}].build = {s:?}; expected `cargo` or `nix-flake`")
        })?,
        None => RepoBuild::default(),
    };
    let attr = read_string(table, "attr").unwrap_or_default();
    let path = read_string(table, "path").unwrap_or_default();

    check_toml_value(&format!("repos.{name}.source"), &source)?;
    check_toml_value(&format!("repos.{name}.pin"), &pin)?;
    check_toml_value(&format!("repos.{name}.attr"), &attr)?;
    check_toml_value(&format!("repos.{name}.path"), &path)?;
    check_repo_source(name, &source)?;

    Ok(Some(RepoRef {
        source,
        pin,
        build,
        attr,
        path,
    }))
}

/// Reject `[repos.<name>].source` values that would let a malicious
/// `scaffold.toml` execute code on contributor machines via `git clone`.
///
/// Two classes are covered here, both reachable from `ensure_repo_present`:
///
/// - Leading `-` is treated by `git clone` as an option, not a positional
///   `<repository>`. Even with the `--` separator the clone call sites pass
///   defensively, parse-time rejection gives a clear error pointing at the
///   offending key instead of a confusing subprocess failure.
/// - `ext::` (and other remote-helper transports written as `<helper>::...`)
///   invoke `git-remote-<helper>`, which for `ext` runs an arbitrary shell
///   command — the CVE-2017-1000117 class. None of scaffold's flows need
///   it, so refusing it at parse time is strictly safer.
fn check_repo_source(name: &str, source: &str) -> DynResult<()> {
    let trimmed = source.trim();
    if trimmed.is_empty() {
        bail!("invalid scaffold.toml: [repos.{name}].source is empty");
    }
    if trimmed.starts_with('-') {
        bail!(
            "invalid scaffold.toml: [repos.{name}].source starts with '-' ({source:?}); \
             refusing — git would treat this as an option, not a repository"
        );
    }
    if is_dangerous_transport(trimmed) {
        bail!(
            "invalid scaffold.toml: [repos.{name}].source uses a dangerous git transport ({source:?}); \
             `ext::` and other remote-helper transports can execute arbitrary commands at clone time and are not allowed"
        );
    }
    Ok(())
}

/// Match the `<helper>::<rest>` remote-helper syntax for transports that can
/// execute code. `ext::` is the canonical RCE vector (CVE-2017-1000117); the
/// rest of the recognized list mirrors transports whose helpers historically
/// shipped shell-out behavior or are otherwise unsuitable for an untrusted
/// `scaffold.toml`.
fn is_dangerous_transport(source: &str) -> bool {
    const BANNED_PREFIXES: &[&str] = &["ext::", "ext ::", "transport-helper::"];
    let lowered = source.to_ascii_lowercase();
    BANNED_PREFIXES
        .iter()
        .any(|prefix| lowered.starts_with(prefix))
}

fn parse_modules(doc: &DocumentMut) -> DynResult<std::collections::BTreeMap<String, ModuleEntry>> {
    let mut out = std::collections::BTreeMap::new();
    let Some(modules) = doc.get("modules").and_then(Item::as_table) else {
        return Ok(out);
    };
    for (name, item) in modules.iter() {
        let table = item
            .as_table()
            .ok_or_else(|| anyhow!("invalid scaffold.toml: [modules.{name}] is not a table"))?;
        let flake = read_string(table, "flake").ok_or_else(|| {
            anyhow!("invalid scaffold.toml: [modules.{name}] missing required field `flake`")
        })?;
        let role_str = read_string(table, "role").unwrap_or_default();
        let role = match role_str.as_str() {
            "project" => ModuleRole::Project,
            "dependency" => ModuleRole::Dependency,
            other => bail!(
                "invalid scaffold.toml: [modules.{name}].role = {other:?}; expected `project` or `dependency`"
            ),
        };
        check_toml_value(&format!("modules.{name}.flake"), &flake)?;
        out.insert(name.to_string(), ModuleEntry { flake, role });
    }
    Ok(out)
}

fn parse_basecamp_runtime(doc: &DocumentMut) -> DynResult<Option<BasecampConfig>> {
    let Some(table) = doc.get("basecamp").and_then(Item::as_table) else {
        return Ok(None);
    };
    // An empty [basecamp] table (e.g. just defaults inherited) still resolves
    // to None — nothing observable distinguishes it from "section omitted",
    // so emit only when the user wrote a non-default value.
    let mut cfg = BasecampConfig::default();
    let mut any_field = false;
    if let Some(v) = table.get("port_base").and_then(Item::as_value) {
        cfg.port_base = v
            .as_integer()
            .and_then(|i| u16::try_from(i).ok())
            .ok_or_else(|| anyhow!("invalid scaffold.toml: [basecamp].port_base must be a u16"))?;
        any_field = true;
    }
    if let Some(v) = table.get("port_stride").and_then(Item::as_value) {
        cfg.port_stride = v
            .as_integer()
            .and_then(|i| u16::try_from(i).ok())
            .ok_or_else(|| {
                anyhow!("invalid scaffold.toml: [basecamp].port_stride must be a u16")
            })?;
        any_field = true;
    }
    Ok(if any_field { Some(cfg) } else { None })
}

fn parse_framework(doc: &DocumentMut) -> FrameworkConfig {
    let table = doc.get("framework").and_then(Item::as_table);
    let kind = table
        .and_then(|t| read_string(t, "kind"))
        .unwrap_or_else(|| FRAMEWORK_KIND_DEFAULT.to_string());
    let version = table
        .and_then(|t| read_string(t, "version"))
        .unwrap_or_else(|| DEFAULT_FRAMEWORK_VERSION.to_string());
    let idl_table = doc
        .get("framework")
        .and_then(|f| f.as_table())
        .and_then(|t| t.get("idl").and_then(Item::as_table));
    let idl_spec = idl_table
        .and_then(|t| read_string(t, "spec"))
        .unwrap_or_else(|| DEFAULT_FRAMEWORK_IDL_SPEC.to_string());
    let idl_path = idl_table
        .and_then(|t| read_string(t, "path"))
        .unwrap_or_else(|| DEFAULT_FRAMEWORK_IDL_PATH.to_string());
    FrameworkConfig {
        kind,
        version,
        idl: FrameworkIdlConfig {
            spec: idl_spec,
            path: idl_path,
        },
    }
}

fn parse_localnet(doc: &DocumentMut) -> DynResult<LocalnetConfig> {
    let mut cfg = LocalnetConfig::default();
    let Some(table) = doc.get("localnet").and_then(Item::as_table) else {
        return Ok(cfg);
    };
    if let Some(v) = table.get("port").and_then(Item::as_value) {
        let int = v
            .as_integer()
            .ok_or_else(|| anyhow!("invalid scaffold.toml: [localnet].port is not an integer"))?;
        cfg.port = u16::try_from(int).map_err(|_| {
            anyhow!(
                "invalid scaffold.toml: [localnet] port `{int}` is not a valid u16 (expected 0-65535)"
            )
        })?;
    }
    if let Some(v) = table.get("risc0_dev_mode").and_then(Item::as_value) {
        cfg.risc0_dev_mode = v.as_bool().unwrap_or(true);
    }
    Ok(cfg)
}

fn read_string(table: &Table, key: &str) -> Option<String> {
    table
        .get(key)
        .and_then(Item::as_str)
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
}

/// Serialize a `Config` to TOML text. Used for fresh writes (`new`, `init`
/// from scratch). For migrations that need to preserve user comments,
/// callers operate on a `DocumentMut` directly via the helpers in
/// `commands::init`.
pub(crate) fn serialize_config(cfg: &Config) -> DynResult<String> {
    let mut doc = DocumentMut::new();

    // [scaffold]
    let scaffold = doc.entry("scaffold").or_insert(Item::Table(Table::new()));
    let scaffold_table = scaffold.as_table_mut().expect("scaffold is a table");
    scaffold_table["version"] = value(&cfg.version);
    if !cfg.cache_root.is_empty() {
        check_toml_value("cache_root", &cfg.cache_root)?;
        scaffold_table["cache_root"] = value(&cfg.cache_root);
    }

    // [repos.<name>] entries — render in stable order.
    write_repo_ref(&mut doc, "lez", &cfg.lez)?;
    write_repo_ref(&mut doc, "spel", &cfg.spel)?;
    if let Some(repo) = &cfg.basecamp_repo {
        write_repo_ref(&mut doc, "basecamp", repo)?;
    }
    if let Some(repo) = &cfg.lgpm_repo {
        write_repo_ref(&mut doc, "lgpm", repo)?;
    }

    // [modules.<name>] entries.
    for (name, entry) in &cfg.modules {
        check_toml_value(&format!("modules.{name}"), name)?;
        check_toml_value(&format!("modules.{name}.flake"), &entry.flake)?;
        let role_str = match entry.role {
            ModuleRole::Project => "project",
            ModuleRole::Dependency => "dependency",
        };
        let path = format!("modules.{name}");
        let table = ensure_subtable(&mut doc, "modules", name);
        table["flake"] = value(&entry.flake);
        table["role"] = value(role_str);
        // Defensive: the function's check above already covered both fields.
        let _ = path;
    }

    // [wallet]
    check_toml_value("wallet.home_dir", &cfg.wallet_home_dir)?;
    let wallet = doc.entry("wallet").or_insert(Item::Table(Table::new()));
    wallet.as_table_mut().expect("wallet table")["home_dir"] = value(&cfg.wallet_home_dir);

    // [framework] / [framework.idl]
    check_toml_value("framework.kind", &cfg.framework.kind)?;
    check_toml_value("framework.version", &cfg.framework.version)?;
    check_toml_value("framework.idl.spec", &cfg.framework.idl.spec)?;
    check_toml_value("framework.idl.path", &cfg.framework.idl.path)?;
    let framework = doc.entry("framework").or_insert(Item::Table(Table::new()));
    let framework_table = framework.as_table_mut().expect("framework table");
    framework_table["kind"] = value(&cfg.framework.kind);
    framework_table["version"] = value(&cfg.framework.version);
    let idl = framework_table
        .entry("idl")
        .or_insert(Item::Table(Table::new()));
    let idl_table = idl.as_table_mut().expect("idl table");
    idl_table["spec"] = value(&cfg.framework.idl.spec);
    idl_table["path"] = value(&cfg.framework.idl.path);

    // [localnet]
    let localnet = doc.entry("localnet").or_insert(Item::Table(Table::new()));
    let localnet_table = localnet.as_table_mut().expect("localnet table");
    localnet_table["port"] = value(i64::from(cfg.localnet.port));
    localnet_table["risc0_dev_mode"] = value(cfg.localnet.risc0_dev_mode);

    // [basecamp]
    if let Some(bc) = &cfg.basecamp {
        let basecamp = doc.entry("basecamp").or_insert(Item::Table(Table::new()));
        let basecamp_table = basecamp.as_table_mut().expect("basecamp table");
        basecamp_table["port_base"] = value(i64::from(bc.port_base));
        basecamp_table["port_stride"] = value(i64::from(bc.port_stride));
    }

    // [run] — only emit when non-default to keep fresh scaffold.toml minimal.
    write_run_config(&mut doc, &cfg.run)?;

    Ok(doc.to_string())
}

fn write_run_config(doc: &mut DocumentMut, run: &RunConfig) -> DynResult<()> {
    let has_inline = run.inline.reset || !run.inline.post_deploy.is_empty();
    let has_default_profile = run.default_profile.is_some();
    let has_profiles = !run.profiles.is_empty();
    if !has_inline && !has_default_profile && !has_profiles {
        return Ok(());
    }

    let run_item = doc.entry("run").or_insert(Item::Table(Table::new()));
    let run_table = run_item.as_table_mut().expect("run table");
    if let Some(name) = &run.default_profile {
        check_toml_value("run.default_profile", name)?;
        run_table["default_profile"] = value(name);
    }
    if run.inline.reset {
        run_table["reset"] = value(true);
    }
    if !run.inline.post_deploy.is_empty() {
        for hook in &run.inline.post_deploy {
            check_toml_value("run.post_deploy", hook)?;
        }
        run_table["post_deploy"] = post_deploy_value(&run.inline.post_deploy);
    }

    if has_profiles {
        for (name, profile) in &run.profiles {
            check_toml_value(&format!("run.profiles.{name}"), name)?;
            for hook in &profile.post_deploy {
                check_toml_value(&format!("run.profiles.{name}.post_deploy"), hook)?;
            }
            let table = ensure_subtable(doc, "run", "profiles");
            // ensure_subtable returns the `profiles` table; we need a
            // sub-sub-table keyed by `name`.
            table.set_implicit(true);
            let profile_table = table
                .entry(name)
                .or_insert(Item::Table(Table::new()))
                .as_table_mut()
                .expect("profile table");
            if profile.reset {
                profile_table["reset"] = value(true);
            }
            if !profile.post_deploy.is_empty() {
                profile_table["post_deploy"] = post_deploy_value(&profile.post_deploy);
            }
        }
    }
    Ok(())
}

fn post_deploy_value(hooks: &[String]) -> Item {
    if hooks.len() == 1 {
        value(&hooks[0])
    } else {
        let mut arr = toml_edit::Array::new();
        for h in hooks {
            arr.push(h.as_str());
        }
        value(arr)
    }
}

fn write_repo_ref(doc: &mut DocumentMut, name: &str, repo: &RepoRef) -> DynResult<()> {
    check_toml_value(&format!("repos.{name}.source"), &repo.source)?;
    check_toml_value(&format!("repos.{name}.pin"), &repo.pin)?;
    check_toml_value(&format!("repos.{name}.attr"), &repo.attr)?;
    check_toml_value(&format!("repos.{name}.path"), &repo.path)?;
    let table = ensure_subtable(doc, "repos", name);
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
    Ok(())
}

fn ensure_subtable<'a>(doc: &'a mut DocumentMut, parent: &str, child: &str) -> &'a mut Table {
    let parent_item = doc.entry(parent).or_insert(Item::Table({
        let mut t = Table::new();
        t.set_implicit(true);
        t
    }));
    let parent_table = parent_item.as_table_mut().expect("parent is a table");
    parent_table.set_implicit(true);
    parent_table
        .entry(child)
        .or_insert(Item::Table(Table::new()))
        .as_table_mut()
        .expect("child is a table")
}

/// Reject any value containing a newline, CR, tab, or other C0 control
/// character. The line-oriented sub-parsers (run profiles, hooks, etc.)
/// elsewhere in the codebase still treat newlines as record separators, so
/// we keep this defense-in-depth even now that toml_edit handles the
/// outer file. Used as a single chokepoint at write time.
pub(crate) fn check_toml_value(key: &str, value: &str) -> DynResult<()> {
    if let Some(bad) = value
        .chars()
        .find(|c| *c == '\n' || *c == '\r' || *c == '\t' || (*c as u32) < 0x20)
    {
        bail!(
            "scaffold.toml `{key}` contains control character {:?} which would \
             corrupt the line-oriented serializer: {value:?}",
            bad
        );
    }
    Ok(())
}

// Convenience for callers who want to construct the canonical default
// `[repos.lez]` / `[repos.spel]` / `[repos.basecamp]` / `[repos.lgpm]`
// entries without duplicating the source/pin/build/attr defaults.
//
// These are intentionally defined here rather than in `model.rs` so that
// `model.rs` stays free of constant references — the defaults live with
// the file format that consumes them.

pub(crate) fn default_lez_repo(pin: &str) -> RepoRef {
    RepoRef {
        source: LEZ_SOURCE.to_string(),
        pin: pin.to_string(),
        build: RepoBuild::Cargo,
        attr: String::new(),
        path: String::new(),
    }
}

pub(crate) fn default_spel_repo(pin: &str) -> RepoRef {
    RepoRef {
        source: SPEL_SOURCE.to_string(),
        pin: pin.to_string(),
        build: RepoBuild::Cargo,
        attr: String::new(),
        path: String::new(),
    }
}

pub(crate) fn default_basecamp_repo(pin: &str) -> RepoRef {
    RepoRef {
        source: BASECAMP_SOURCE.to_string(),
        pin: pin.to_string(),
        build: RepoBuild::NixFlake,
        attr: BASECAMP_ATTR.to_string(),
        path: String::new(),
    }
}

pub(crate) fn default_lgpm_repo(pin: &str) -> RepoRef {
    RepoRef {
        source: LGPM_SOURCE.to_string(),
        pin: pin.to_string(),
        build: RepoBuild::NixFlake,
        attr: LGPM_ATTR.to_string(),
        path: String::new(),
    }
}

// The old `parse_inline_string_array`, `unquote`, and `escape_toml_string`
// helpers are no longer needed — toml_edit handles array parsing, quote
// unwrapping, and string escaping for `value(..)` calls. The hand-rolled
// preserving emitter is gone along with them.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::{DEFAULT_BASECAMP_PIN, DEFAULT_LEZ, DEFAULT_LGPM_PIN, DEFAULT_SPEL};

    fn base_config() -> Config {
        parse_config(&minimal_v0_2_0()).expect("parse minimal v0.2.0")
    }

    fn minimal_v0_2_0() -> String {
        format!(
            r#"[scaffold]
version = "0.2.0"

[repos.lez]
source = "{lez_src}"
pin = "{lez_pin}"

[repos.spel]
source = "{spel_src}"
pin = "{spel_pin}"

[wallet]
home_dir = ".scaffold/wallet"

[framework]
kind = "default"
version = "0.1.0"

[framework.idl]
spec = "lssa-idl/0.1.0"
path = "idl"

[localnet]
port = 3040
risc0_dev_mode = true
"#,
            lez_src = LEZ_SOURCE,
            lez_pin = DEFAULT_LEZ.sha,
            spel_src = SPEL_SOURCE,
            spel_pin = DEFAULT_SPEL.sha,
        )
    }

    #[test]
    fn parses_minimal_v0_2_0() {
        let cfg = parse_config(&minimal_v0_2_0()).expect("parse");
        assert_eq!(cfg.version, SCAFFOLD_TOML_SCHEMA_VERSION);
        assert_eq!(cfg.lez.source, LEZ_SOURCE);
        assert_eq!(cfg.lez.pin, DEFAULT_LEZ.sha);
        assert_eq!(cfg.lez.build, RepoBuild::Cargo);
        assert!(cfg.lez.attr.is_empty());
        assert!(cfg.lez.path.is_empty());
        assert!(cfg.basecamp_repo.is_none());
        assert!(cfg.lgpm_repo.is_none());
        assert!(cfg.modules.is_empty());
        assert!(cfg.basecamp.is_none());
    }

    #[test]
    fn parses_repos_basecamp_with_nix_flake() {
        let toml = minimal_v0_2_0()
            + &format!(
                r#"
[repos.basecamp]
source = "{}"
pin = "{}"
build = "nix-flake"
attr = "app"

[repos.lgpm]
source = "{}"
pin = "{}"
build = "nix-flake"
attr = "cli"
"#,
                BASECAMP_SOURCE, DEFAULT_BASECAMP_PIN, LGPM_SOURCE, DEFAULT_LGPM_PIN,
            );
        let cfg = parse_config(&toml).expect("parse");
        let bc = cfg.basecamp_repo.expect("basecamp present");
        assert_eq!(bc.build, RepoBuild::NixFlake);
        assert_eq!(bc.attr, "app");
        let lgpm = cfg.lgpm_repo.expect("lgpm present");
        assert_eq!(lgpm.build, RepoBuild::NixFlake);
        assert_eq!(lgpm.attr, "cli");
    }

    #[test]
    fn parses_modules_section() {
        let toml = minimal_v0_2_0()
            + r#"
[modules.tictactoe]
flake = "path:./tictactoe"
role = "project"

[modules.delivery_module]
flake = "github:logos-co/logos-delivery-module/abc#lgx"
role = "dependency"
"#;
        let cfg = parse_config(toml.as_str()).expect("parse");
        assert_eq!(cfg.modules.len(), 2);
        let tic = cfg.modules.get("tictactoe").expect("tic");
        assert_eq!(tic.flake, "path:./tictactoe");
        assert_eq!(tic.role, ModuleRole::Project);
        let dm = cfg.modules.get("delivery_module").expect("dm");
        assert_eq!(dm.role, ModuleRole::Dependency);
    }

    #[test]
    fn rejects_basecamp_pin_field_with_init_hint() {
        let toml = minimal_v0_2_0()
            + r#"
[basecamp]
pin = "deadbeef"
source = "https://example/basecamp"
"#;
        let err = parse_config(&toml).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("logos-scaffold init"), "{msg}");
        assert!(
            msg.contains("[basecamp]") || msg.contains("[repos.basecamp]"),
            "{msg}"
        );
    }

    #[test]
    fn rejects_basecamp_modules_legacy_with_init_hint() {
        let toml = minimal_v0_2_0()
            + r#"
[basecamp.modules.foo]
flake = "path:./foo"
role = "project"
"#;
        let err = parse_config(&toml).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("[modules"), "{msg}");
        assert!(msg.contains("logos-scaffold init"), "{msg}");
    }

    #[test]
    fn rejects_repos_lez_url_field_with_init_hint() {
        let mut toml = minimal_v0_2_0();
        // Inject `url = "..."` into [repos.lez].
        toml = toml.replace(
            "[repos.lez]\nsource",
            "[repos.lez]\nurl = \"https://example/lez.git\"\nsource",
        );
        let err = parse_config(&toml).unwrap_err();
        assert!(err.to_string().contains("logos-scaffold init"), "{err}");
    }

    #[test]
    fn rejects_pre_v0_2_0_version() {
        let toml = minimal_v0_2_0().replace("version = \"0.2.0\"", "version = \"0.1.1\"");
        let err = parse_config(&toml).unwrap_err();
        assert!(err.to_string().contains("logos-scaffold init"), "{err}");
    }

    #[test]
    fn round_trips_through_serialize() {
        let cfg1 = parse_config(&minimal_v0_2_0()).expect("parse");
        let serialized = serialize_config(&cfg1).expect("serialize");
        let cfg2 = parse_config(&serialized).expect("re-parse");
        assert_eq!(cfg2.version, cfg1.version);
        assert_eq!(cfg2.lez.source, cfg1.lez.source);
        assert_eq!(cfg2.lez.pin, cfg1.lez.pin);
        assert_eq!(cfg2.spel.pin, cfg1.spel.pin);
    }

    #[test]
    fn serialize_omits_default_build_and_empty_optional_fields() {
        let cfg = parse_config(&minimal_v0_2_0()).expect("parse");
        let serialized = serialize_config(&cfg).expect("serialize");
        // [repos.lez] is cargo-built with no attr/path; nothing besides
        // source and pin should appear.
        assert!(!serialized.contains("build = \"cargo\""), "{serialized}");
        assert!(!serialized.contains("attr ="), "{serialized}");
        // path = "" should not be persisted.
        for line in serialized.lines() {
            assert!(line.trim() != "path = \"\"", "{serialized}");
        }
    }

    #[test]
    fn serialize_emits_path_when_set() {
        let mut cfg = parse_config(&minimal_v0_2_0()).expect("parse");
        cfg.lez.path = "/abs/lez".to_string();
        let serialized = serialize_config(&cfg).expect("serialize");
        assert!(serialized.contains("path = \"/abs/lez\""), "{serialized}");
    }

    #[test]
    fn serialize_emits_no_url_field_anywhere() {
        let cfg = parse_config(&minimal_v0_2_0()).expect("parse");
        let serialized = serialize_config(&cfg).expect("serialize");
        assert!(
            !serialized.contains("url ="),
            "url field should not be emitted in 0.2.0 schema:\n{serialized}"
        );
    }

    #[test]
    fn check_toml_value_rejects_newline() {
        assert!(check_toml_value("k", "a\nb").is_err());
    }

    #[test]
    fn rejects_legacy_repos_lssa_section() {
        let toml = minimal_v0_2_0().replace("[repos.lez]", "[repos.lssa]");
        let err = parse_config(&toml).expect_err("lssa section should be rejected");
        let msg = err.to_string();
        assert!(msg.contains("lssa"), "{msg}");
        assert!(msg.contains("init"), "{msg}");
    }

    #[test]
    fn parse_localnet_port_out_of_range_errors() {
        let toml = minimal_v0_2_0().replace("port = 3040", "port = 70000");
        let err = parse_config(&toml).unwrap_err();
        assert!(
            err.to_string().contains("70000") || err.to_string().contains("u16"),
            "{err}"
        );
    }

    #[test]
    fn rejects_repo_source_starting_with_dash() {
        let toml = minimal_v0_2_0().replace(
            &format!("source = \"{}\"\npin = \"{}\"", LEZ_SOURCE, DEFAULT_LEZ.sha),
            &format!(
                "source = \"-upload-pack=evil\"\npin = \"{}\"",
                DEFAULT_LEZ.sha
            ),
        );
        let err = parse_config(&toml).expect_err("dash-prefixed source must be rejected");
        let msg = err.to_string();
        assert!(msg.contains("repos.lez"), "{msg}");
        assert!(msg.contains("starts with '-'"), "{msg}");
    }

    #[test]
    fn rejects_repo_source_with_ext_transport() {
        let toml = minimal_v0_2_0().replace(
            &format!("source = \"{}\"\npin = \"{}\"", LEZ_SOURCE, DEFAULT_LEZ.sha),
            &format!("source = \"ext::sh -c id\"\npin = \"{}\"", DEFAULT_LEZ.sha),
        );
        let err = parse_config(&toml).expect_err("ext:: transport must be rejected");
        let msg = err.to_string();
        assert!(msg.contains("repos.lez"), "{msg}");
        assert!(msg.contains("dangerous git transport"), "{msg}");
    }

    #[test]
    fn rejects_repo_source_with_ext_transport_case_insensitive() {
        let toml = minimal_v0_2_0().replace(
            &format!("source = \"{}\"\npin = \"{}\"", LEZ_SOURCE, DEFAULT_LEZ.sha),
            &format!("source = \"EXT::sh -c id\"\npin = \"{}\"", DEFAULT_LEZ.sha),
        );
        let err = parse_config(&toml).expect_err("upper-case ext:: must be rejected");
        assert!(err.to_string().contains("dangerous git transport"), "{err}");
    }

    #[test]
    fn rejects_repo_source_with_transport_helper_prefix() {
        let toml = minimal_v0_2_0().replace(
            &format!("source = \"{}\"\npin = \"{}\"", LEZ_SOURCE, DEFAULT_LEZ.sha),
            &format!(
                "source = \"transport-helper::evil\"\npin = \"{}\"",
                DEFAULT_LEZ.sha
            ),
        );
        let err = parse_config(&toml).expect_err("transport-helper:: must be rejected");
        assert!(err.to_string().contains("dangerous git transport"), "{err}");
    }

    #[test]
    fn accepts_ordinary_repo_sources() {
        // Defense-in-depth: the rejection path is selective. Confirm the
        // common, benign source shapes still parse — https, ssh, git@, plain
        // paths.
        for source in [
            "https://github.com/example/repo.git",
            "http://example.com/repo",
            "ssh://git@example.com/repo.git",
            "git@github.com:example/repo.git",
            "/abs/local/repo",
            "./relative/repo",
            "extender/repo",
        ] {
            let toml = minimal_v0_2_0().replace(
                &format!("source = \"{}\"\npin = \"{}\"", LEZ_SOURCE, DEFAULT_LEZ.sha),
                &format!("source = \"{}\"\npin = \"{}\"", source, DEFAULT_LEZ.sha),
            );
            parse_config(&toml)
                .unwrap_or_else(|e| panic!("benign source {source:?} rejected: {e}"));
        }
    }

    #[test]
    fn parses_path_override_for_back_compat() {
        let toml = minimal_v0_2_0().replace(
            "[repos.lez]\nsource",
            "[repos.lez]\npath = \"/abs/lez\"\nsource",
        );
        let cfg = parse_config(&toml).expect("parse");
        assert_eq!(cfg.lez.path, "/abs/lez");
    }

    #[test]
    fn parse_config_with_run_profile_subsection() {
        let toml = minimal_v0_2_0()
            + "[run.profiles.e2e]\nreset = true\npost_deploy = [\"scripts/e2e.sh\"]\n";
        let cfg = parse_config(&toml).expect("parse");
        let prof = cfg.run.profiles.get("e2e").expect("e2e present");
        assert!(prof.reset);
        assert_eq!(prof.post_deploy, vec!["scripts/e2e.sh".to_string()]);
    }

    #[test]
    fn parse_config_default_profile_must_exist() {
        let toml = minimal_v0_2_0() + "[run]\ndefault_profile = \"missing\"\n";
        let err = parse_config(&toml).unwrap_err();
        assert!(
            err.to_string().contains("missing")
                && err.to_string().contains("[run.profiles.missing]"),
            "{err}"
        );
    }

    #[test]
    fn parse_config_default_profile_resolves() {
        let toml = minimal_v0_2_0()
            + "[run]\ndefault_profile = \"play\"\n[run.profiles.play]\npost_deploy = \"echo play\"\n";
        let cfg = parse_config(&toml).expect("parse");
        assert_eq!(cfg.run.default_profile.as_deref(), Some("play"));
        let resolved = cfg.run.resolve_profile(None).expect("resolve");
        assert_eq!(resolved.post_deploy, vec!["echo play".to_string()]);
    }

    #[test]
    fn resolve_profile_explicit_selector_wins() {
        let toml = minimal_v0_2_0()
            + "[run]\npost_deploy = [\"echo inline\"]\ndefault_profile = \"play\"\n[run.profiles.play]\npost_deploy = \"echo play\"\n[run.profiles.e2e]\npost_deploy = \"echo e2e\"\n";
        let cfg = parse_config(&toml).expect("parse");
        let r = cfg.run.resolve_profile(Some("e2e")).expect("resolve");
        assert_eq!(r.post_deploy, vec!["echo e2e".to_string()]);
    }

    #[test]
    fn resolve_profile_unknown_name_errors_with_known_list() {
        let toml = minimal_v0_2_0()
            + "[run.profiles.play]\npost_deploy = \"echo play\"\n[run.profiles.e2e]\npost_deploy = \"echo e2e\"\n";
        let cfg = parse_config(&toml).expect("parse");
        let err = cfg.run.resolve_profile(Some("missing")).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("missing"), "{msg}");
        assert!(msg.contains("play") && msg.contains("e2e"), "{msg}");
    }

    #[test]
    fn resolve_profile_falls_back_to_inline_when_no_default() {
        let toml = minimal_v0_2_0() + "[run]\nreset = true\n";
        let cfg = parse_config(&toml).expect("parse");
        let r = cfg.run.resolve_profile(None).expect("resolve");
        assert!(r.reset);
        assert!(r.post_deploy.is_empty());
    }

    /// When `[run].default_profile` resolves, inline `[run]` values are
    /// fully shadowed — they do not merge. Mirrors the `--profile X`
    /// behavior so the two ways of selecting a profile have identical
    /// semantics.
    #[test]
    fn resolve_profile_default_profile_fully_shadows_inline() {
        let toml = minimal_v0_2_0()
            + "[run]\ndefault_profile = \"dev\"\npost_deploy = [\"echo inline\"]\nreset = true\n[run.profiles.dev]\npost_deploy = [\"echo dev\"]\n";
        let cfg = parse_config(&toml).expect("parse");
        let r = cfg.run.resolve_profile(None).expect("resolve");
        assert_eq!(r.post_deploy, vec!["echo dev".to_string()]);
        assert!(
            !r.reset,
            "inline reset must not bleed into resolved profile"
        );
    }

    #[test]
    fn run_profiles_round_trip_through_parse_serialize() {
        let toml = minimal_v0_2_0()
            + "[run]\ndefault_profile = \"dev\"\n[run.profiles.dev]\npost_deploy = [\"echo dev\"]\n[run.profiles.e2e]\nreset = true\npost_deploy = [\"echo e2e\"]\n";
        let cfg1 = parse_config(&toml).expect("parse");
        let serialized = serialize_config(&cfg1).expect("serialize");
        let cfg2 = parse_config(&serialized).expect("re-parse");
        assert_eq!(cfg2.run.default_profile.as_deref(), Some("dev"));
        assert_eq!(cfg2.run.profiles.len(), 2);
        let e2e = cfg2.run.profiles.get("e2e").expect("e2e");
        assert!(e2e.reset);
        assert_eq!(e2e.post_deploy, vec!["echo e2e".to_string()]);
    }

    #[test]
    fn serialize_rejects_newline_in_profile_post_deploy() {
        let mut cfg = base_config();
        let mut profiles = std::collections::BTreeMap::new();
        profiles.insert(
            "dev".to_string(),
            RunProfile {
                reset: false,
                post_deploy: vec!["echo a\n[run.profiles.evil]".to_string()],
            },
        );
        cfg.run = RunConfig {
            profiles,
            ..RunConfig::default()
        };
        let err = serialize_config(&cfg).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("post_deploy") && msg.contains("dev"), "{msg}");
    }
}
