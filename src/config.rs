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
//!   `[framework]`, `[localnet]`, `[circuits]`, `[basecamp]`
//!   (port allocation only).
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
    BasecampConfig, BasecampProfile, CircuitsConfig, Config, FrameworkConfig, FrameworkIdlConfig,
    LocalnetConfig, ModuleEntry, ModuleRole, RepoBuild, RepoRef, RunConfig, RunProfile,
    WatchConfig,
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
    let circuits = parse_circuits(&doc)?;
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
        circuits,
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
    let inline_deploy = run_table
        .get("deploy")
        .and_then(Item::as_value)
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let inline_topup = run_table
        .get("topup")
        .and_then(Item::as_value)
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
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
            let deploy = table
                .get("deploy")
                .and_then(Item::as_value)
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let topup = table
                .get("topup")
                .and_then(Item::as_value)
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let post_deploy = parse_post_deploy(table.get("post_deploy"))?;
            profiles.insert(
                name.to_string(),
                RunProfile {
                    reset,
                    post_deploy,
                    deploy,
                    topup,
                },
            );
        }
    }

    if let Some(name) = &default_profile {
        if !profiles.contains_key(name) {
            bail!(
                "invalid scaffold.toml: [run].default_profile = {name:?} but no [run.profiles.{name}] section"
            );
        }
    }

    let watch = parse_run_watch(run_table)?;

    Ok(RunConfig {
        default_profile,
        inline: RunProfile {
            reset: inline_reset,
            post_deploy: inline_post_deploy,
            deploy: inline_deploy,
            topup: inline_topup,
        },
        profiles,
        watch,
    })
}

fn parse_run_watch(run_table: &Table) -> DynResult<WatchConfig> {
    let Some(watch_table) = run_table.get("watch").and_then(Item::as_table) else {
        return Ok(WatchConfig::default());
    };
    let include = parse_glob_list(watch_table.get("include"), "[run.watch].include")?;
    let exclude = parse_glob_list(watch_table.get("exclude"), "[run.watch].exclude")?;
    let debounce_ms = match watch_table.get("debounce_ms") {
        None => None,
        Some(item) => {
            let n = item.as_integer().ok_or_else(|| {
                anyhow!("invalid scaffold.toml: [run.watch].debounce_ms must be an integer")
            })?;
            if n < 0 {
                bail!("invalid scaffold.toml: [run.watch].debounce_ms must be non-negative");
            }
            Some(n as u64)
        }
    };
    Ok(WatchConfig {
        include,
        exclude,
        debounce_ms,
    })
}

/// `key` is the field label already formatted as `[table].field` (e.g.
/// `[run.watch].include`), so error messages point at the actual key instead of
/// a `[run.watch.include]`-looking pseudo-table.
fn parse_glob_list(item: Option<&Item>, key: &str) -> DynResult<Vec<String>> {
    let Some(item) = item else {
        return Ok(Vec::new());
    };
    let arr = item
        .as_array()
        .ok_or_else(|| anyhow!("invalid scaffold.toml: {key} must be an array of strings"))?;
    let mut out = Vec::with_capacity(arr.len());
    for v in arr.iter() {
        let s = v
            .as_str()
            .ok_or_else(|| anyhow!("invalid scaffold.toml: {key} entries must be strings"))?;
        // Reject empty patterns: an empty glob normalizes to a match-all
        // (`**/`), so an empty `exclude` entry would silently suppress *every*
        // watch trigger. Fail fast with a targeted error instead.
        if s.is_empty() {
            bail!("invalid scaffold.toml: {key} entries must not be empty");
        }
        out.push(s.to_string());
    }
    Ok(out)
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

/// Per-shape markers returned by `detect_old_schema_markers`. The
/// user-facing error doesn't enumerate these — they're a structured signal
/// for tests and any future verbose log path.
#[derive(Debug, Default, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct OldSchemaMarkers {
    pub(crate) version_stale: bool,
    pub(crate) has_lssa: bool,
    pub(crate) has_repo_url: bool,
    pub(crate) has_old_basecamp_keys: bool,
    pub(crate) has_old_basecamp_modules: bool,
}

impl OldSchemaMarkers {
    pub(crate) fn any(&self) -> bool {
        self.version_stale
            || self.has_lssa
            || self.has_repo_url
            || self.has_old_basecamp_keys
            || self.has_old_basecamp_modules
    }
}

/// Pragmatic detection of pre-0.2.0 schemas. Returns a flag-per-shape so the
/// caller can decide what (if anything) to surface. `init`'s migrator handles
/// every variant we detect here, so the user-facing error in
/// `detect_old_schema` does not enumerate them.
pub(crate) fn detect_old_schema_markers(doc: &DocumentMut, version: &str) -> OldSchemaMarkers {
    let mut m = OldSchemaMarkers::default();

    // Old version stamp. Other version mismatches (prerelease tags, hand-edits)
    // are caught downstream in `parse_config` with a more specific "this build
    // expects X" message; `init`'s migrator bumps the version regardless of
    // origin.
    m.version_stale = version != SCAFFOLD_TOML_SCHEMA_VERSION
        && (version.starts_with("0.1.") || version == "0.1" || version == "0.0");

    let repos_table = doc.get("repos").and_then(Item::as_table);
    // [repos.lssa] — pre-spel-era alias for [repos.lez].
    m.has_lssa = repos_table.is_some_and(|t| t.get("lssa").is_some());
    // [repos.{lez,spel}].url — dropped in 0.2.0; source is the single field.
    m.has_repo_url = ["lez", "spel"].iter().any(|name| {
        repos_table
            .and_then(|t| t.get(name).and_then(Item::as_table))
            .is_some_and(|tbl| tbl.get("url").is_some())
    });
    let basecamp_table = doc.get("basecamp").and_then(Item::as_table);
    // Old [basecamp] shape: pin / source / lgpm_flake at the root.
    m.has_old_basecamp_keys = basecamp_table.is_some_and(|t| {
        ["pin", "source", "lgpm_flake"]
            .iter()
            .any(|k| t.get(k).is_some())
    });
    // [basecamp.modules.*] — moved to [modules.*].
    m.has_old_basecamp_modules = basecamp_table
        .and_then(|t| t.get("modules").and_then(Item::as_table))
        .is_some_and(|m| m.iter().next().is_some());

    m
}

/// Reject pre-0.2.0 schemas with a one-line, action-only error pointing at
/// `init`. The migrator handles every variant we detect, so the user only
/// needs to know that a migration is required — not which specific shape
/// tripped the check.
fn detect_old_schema(doc: &DocumentMut, version: &str) -> DynResult<()> {
    if !detect_old_schema_markers(doc, version).any() {
        return Ok(());
    }
    bail!(
        "scaffold.toml uses an old schema. \
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
    // `attr` is either a scalar (`attr = "app"`) or a per-platform map
    // (`[repos.<name>.attr]` / inline `attr = { aarch64-darwin = "…" }`).
    // `read_string` returns None for the table form, leaving `attr` empty.
    let attr = read_string(table, "attr").unwrap_or_default();
    let attr_platform = parse_attr_platform(table, name)?;
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
        attr_platform,
        path,
    }))
}

/// Parse a per-platform `[repos.<name>.attr]` map. Returns an empty map when
/// `attr` is absent or given in scalar form (handled by the caller's
/// `read_string`). Keys are nix system triples (`aarch64-darwin`, etc.).
fn parse_attr_platform(
    repo_table: &Table,
    name: &str,
) -> DynResult<std::collections::BTreeMap<String, String>> {
    let mut out = std::collections::BTreeMap::new();
    let Some(tbl) = repo_table.get("attr").and_then(Item::as_table_like) else {
        return Ok(out);
    };
    for (system, v) in tbl.iter() {
        if system.is_empty() {
            bail!("invalid scaffold.toml: [repos.{name}.attr] has an empty system key");
        }
        // Validate the key, not just the value: a quoted TOML key carrying
        // control characters would otherwise corrupt the line-oriented
        // serializer on the next `save_project_config`.
        check_toml_value(&format!("repos.{name}.attr system key {system:?}"), system)?;
        let s = v.as_str().ok_or_else(|| {
            anyhow!("invalid scaffold.toml: [repos.{name}.attr].{system} must be a string")
        })?;
        check_toml_value(&format!("repos.{name}.attr.{system}"), s)?;
        out.insert(system.to_string(), s.to_string());
    }
    Ok(out)
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
        let standalone_app = read_string(table, "standalone_app").filter(|s| !s.is_empty());
        if let Some(app) = &standalone_app {
            check_toml_value(&format!("modules.{name}.standalone_app"), app)?;
        }
        out.insert(
            name.to_string(),
            ModuleEntry {
                flake,
                role,
                standalone_app,
            },
        );
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

    // [basecamp.env] — plain string map.
    if let Some(env_table) = table.get("env").and_then(Item::as_table) {
        cfg.env = parse_string_map(env_table, "basecamp.env")?;
        any_field = any_field || !cfg.env.is_empty();
    }
    // [basecamp.env_append] — map of string -> array<string>.
    if let Some(append_table) = table.get("env_append").and_then(Item::as_table) {
        for (key, item) in append_table.iter() {
            validate_env_var_name(key, "basecamp.env_append")?;
            let arr = item.as_array().ok_or_else(|| {
                anyhow!("invalid scaffold.toml: [basecamp.env_append].{key} must be an array of strings")
            })?;
            let mut list = Vec::with_capacity(arr.len());
            for v in arr.iter() {
                let s = v.as_str().ok_or_else(|| {
                    anyhow!("invalid scaffold.toml: [basecamp.env_append].{key} entries must be strings")
                })?;
                // Reject empty entries: `:`-joining them yields an empty path
                // segment (e.g. `LD_LIBRARY_PATH=:`), which silently injects the
                // current directory into search paths — surprising and unsafe.
                if s.is_empty() {
                    bail!("invalid scaffold.toml: [basecamp.env_append].{key} entries must not be empty");
                }
                list.push(s.to_string());
            }
            // Skip empty lists: they're a no-op at launch (apply_launch_env_
            // overrides skips them) and would otherwise make `[basecamp]`
            // non-empty and round-trip back into scaffold.toml — inconsistent
            // with how empty per-profile env maps are dropped below.
            if !list.is_empty() {
                cfg.env_append.insert(key.to_string(), list);
            }
        }
        any_field = any_field || !cfg.env_append.is_empty();
    }
    // [basecamp.profiles.<name>] — per-profile launch config.
    if let Some(profiles) = table.get("profiles").and_then(Item::as_table) {
        for (name, item) in profiles.iter() {
            let ptable = item.as_table().ok_or_else(|| {
                anyhow!("invalid scaffold.toml: [basecamp.profiles.{name}] is not a table")
            })?;
            let mut profile = BasecampProfile::default();
            if let Some(env_table) = ptable.get("env").and_then(Item::as_table) {
                profile.env =
                    parse_string_map(env_table, &format!("basecamp.profiles.{name}.env"))?;
            }
            profile.env_file = read_string(ptable, "env_file");
            if let Some(f) = &profile.env_file {
                check_toml_value(&format!("basecamp.profiles.{name}.env_file"), f)?;
            }
            profile.runtime_dir = read_string(ptable, "runtime_dir");
            if let Some(d) = &profile.runtime_dir {
                check_toml_value(&format!("basecamp.profiles.{name}.runtime_dir"), d)?;
            }
            profile.log_file = read_string(ptable, "log_file");
            if let Some(l) = &profile.log_file {
                check_toml_value(&format!("basecamp.profiles.{name}.log_file"), l)?;
            }
            // Drop fully-default profiles so an empty `[basecamp.profiles.foo]`
            // doesn't make `[basecamp]` non-empty and round-trip back.
            if profile != BasecampProfile::default() {
                cfg.profiles.insert(name.to_string(), profile);
            }
        }
        any_field = any_field || !cfg.profiles.is_empty();
    }

    Ok(if any_field { Some(cfg) } else { None })
}

fn parse_string_map(
    table: &Table,
    key: &str,
) -> DynResult<std::collections::BTreeMap<String, String>> {
    let mut out = std::collections::BTreeMap::new();
    for (k, item) in table.iter() {
        validate_env_var_name(k, key)?;
        let v = item
            .as_str()
            .ok_or_else(|| anyhow!("invalid scaffold.toml: [{key}].{k} must be a string"))?;
        out.insert(k.to_string(), v.to_string());
    }
    Ok(out)
}

/// Reject env var names that would only surface as an opaque `exec` /
/// `Command::env` failure at launch: TOML quoted keys can be empty or contain
/// `=` or control characters. Fail fast at parse with an actionable message.
fn validate_env_var_name(name: &str, context: &str) -> DynResult<()> {
    if name.is_empty() {
        bail!("invalid scaffold.toml: [{context}] env var name must not be empty");
    }
    if name.contains('=') {
        bail!("invalid scaffold.toml: [{context}] env var name {name:?} must not contain `=`");
    }
    if name.chars().any(char::is_control) {
        bail!(
            "invalid scaffold.toml: [{context}] env var name {name:?} must not contain control characters"
        );
    }
    Ok(())
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

fn parse_circuits(doc: &DocumentMut) -> DynResult<CircuitsConfig> {
    let Some(table) = doc.get("circuits").and_then(Item::as_table) else {
        return Ok(CircuitsConfig::default());
    };

    let version = read_string(table, "version")
        .ok_or_else(|| anyhow!("invalid scaffold.toml: missing [circuits].version"))?;
    let url_template = read_string(table, "url_template");
    let install_dir =
        read_string(table, "install_dir").unwrap_or_else(|| ".scaffold/circuits".to_string());

    check_toml_value("circuits.version", &version)?;
    if let Some(template) = &url_template {
        check_toml_value("circuits.url_template", template)?;
        check_circuits_url_template(template)?;
    }
    check_toml_value("circuits.install_dir", &install_dir)?;
    // A relative `install_dir` is joined onto the project root (an absolute one
    // is used as-is — see `circuits_install_dir`) and handed to `create_dir_all`
    // + tarball extraction; a `..` component would let the config write outside
    // the project. Reject parent-dir traversal.
    if std::path::Path::new(&install_dir)
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        bail!(
            "invalid scaffold.toml: [circuits].install_dir must not contain `..` \
             components (would escape the project root): {install_dir:?}"
        );
    }

    Ok(CircuitsConfig {
        version,
        url_template,
        install_dir,
    })
}

fn check_circuits_url_template(template: &str) -> DynResult<()> {
    let scheme = template
        .split_once("://")
        .map(|(scheme, _)| scheme)
        .unwrap_or_default();
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        bail!(
            "invalid scaffold.toml: [circuits].url_template must use http:// or https://: \
             {template:?}"
        );
    }
    Ok(())
}

fn read_string(table: &Table, key: &str) -> Option<String> {
    table
        .get(key)
        .and_then(Item::as_str)
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
}

/// Render `cfg` as a fresh `scaffold.toml`. For rewriting a file that already
/// exists, prefer [`update_config`] — it keeps the user's comments.
pub(crate) fn serialize_config(cfg: &Config) -> DynResult<String> {
    write_config_into(DocumentMut::new(), cfg)
}

/// Rewrite an existing `scaffold.toml` in place, preserving comments, key
/// ordering, and any sections scaffold does not model.
///
/// `serialize_config` renders from an empty document, so every comment in the
/// file is dropped on the next write. That matters because scaffold.toml
/// comments are frequently load-bearing — they record *why* a `runtime_dir`
/// override or a pin is what it is — and losing them silently re-opens the bug
/// they were written to prevent. Seeding the same writers with the parsed
/// original leaves untouched keys, their formatting, and their comments
/// exactly where they were.
///
/// **The contract this imposes on [`write_config_into`]:** seeded with a real
/// document, assigning a key is no longer the same as rendering the model.
/// Every optional key must be *removed* when the config no longer carries it,
/// because skipping the assignment now leaves the old value in the file rather
/// than omitting it. A conditional emit without a matching removal branch is a
/// silent bug — the config read back is not the one that was written — so any
/// new `if non_default { assign }` needs its `else { remove }`. The tests pin
/// this by comparing a rewrite against a fresh render for a config whose
/// optional sections have all been reset to defaults.
///
/// Falls back to a from-scratch render if `existing` doesn't parse: a
/// hand-edit that broke the file shouldn't block writing a valid one.
///
/// That fallback is *lossy* — it is exactly the comment-and-unmodelled-section
/// loss this function exists to prevent — so it must never happen quietly.
/// Production code therefore calls [`update_config_reporting`] and acts on the
/// [`RewriteOutcome`] it returns; this wrapper discards that signal and is
/// test-only so no disk-writing path can pick it up by accident.
#[cfg(test)]
pub(crate) fn update_config(existing: &str, cfg: &Config) -> DynResult<String> {
    Ok(update_config_reporting(existing, cfg)?.0)
}

/// How [`update_config_reporting`] produced its output.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum RewriteOutcome {
    /// `existing` parsed; the rewrite merged into it and kept its comments.
    Merged,
    /// `existing` did not parse as TOML, so the output is a from-scratch
    /// render. Every comment and every unmodelled section in `existing` is
    /// absent from it. `reason` is the parse error, for the diagnostic.
    RenderedFresh { reason: String },
}

impl RewriteOutcome {
    pub(crate) fn discarded_reason(&self) -> Option<&str> {
        match self {
            Self::Merged => None,
            Self::RenderedFresh { reason } => Some(reason),
        }
    }
}

/// [`update_config`], but reporting whether the comment-preserving merge
/// actually happened.
///
/// The fallback is deliberate — a file one stray bracket from valid should not
/// wedge every command that persists config — but it replaces the user's file
/// wholesale, so a caller writing the result to disk owes them a warning and,
/// where it can, a copy of what it is about to discard.
pub(crate) fn update_config_reporting(
    existing: &str,
    cfg: &Config,
) -> DynResult<(String, RewriteOutcome)> {
    let (doc, outcome) = match existing.parse::<DocumentMut>() {
        Ok(doc) => (doc, RewriteOutcome::Merged),
        Err(e) => (
            DocumentMut::new(),
            RewriteOutcome::RenderedFresh {
                reason: e.to_string(),
            },
        ),
    };
    Ok((write_config_into(doc, cfg)?, outcome))
}

/// Render `cfg` into `doc`, which may be empty ([`serialize_config`]) or the
/// parsed existing file ([`update_config`]).
///
/// Because `doc` may already hold values, this is a *merge*, not a render: an
/// optional key that is skipped rather than assigned keeps whatever the file
/// already said. So every conditional emit here needs a matching removal —
/// `if non_default { assign } else { remove }` — and every map-backed table
/// (`modules`, `basecamp.env`, `profiles`, `run.profiles`, …) needs a `retain`
/// down to the keys the config still carries. Omitting either is silent: the
/// file keeps a value the caller cleared, and the next load reads it back.
///
/// The inverse is equally a rule, and the sharper one, because getting it
/// wrong destroys data rather than staling it. **A removal may only be driven
/// by the model where the corresponding parser is _total_** — where it errors
/// on anything it cannot represent, so that "absent from the model" really
/// does mean "absent from the file". Several of scaffold's parsers are not:
///
/// - `parse_basecamp_runtime` returns `None` for a `[basecamp]` whose keys are
///   all unmodelled, *skips* an `env_append` entry whose list is empty, and
///   *drops* a `[basecamp.profiles.<name>]` that is fully default.
/// - `parse_run` maps an all-default `[run]` (or `[run.watch]`) to the default
///   value, indistinguishable from absent.
/// - `read_string` filters empty strings, and `parse_post_deploy` /
///   `parse_glob_list` map an empty string or array to an empty `Vec` — so
///   `path = ""`, `env_file = ""`, `post_deploy = ""` and `exclude = []` all
///   reach the model as exactly the state that means "key absent".
///
/// For those, the model's key set is a strict subset of the file's, so a
/// `doc.remove(section)` or a `retain` keyed on the model deletes whatever the
/// user wrote there — the key, the section, and the comment explaining it.
/// Instead clear the keys scaffold owns and keep whatever survives:
/// [`remove_owned_child`], [`clear_owned_basecamp_keys`],
/// [`prune_run_profiles`] and [`remove_unless_empty_literal`] do this.
///
/// Note the second and third bullets compose: a section can parse to `None`
/// *because* each of its fields was individually skipped, so clearing that
/// section must recurse with the same scoped rules rather than dropping its
/// child tables — see [`clear_owned_basecamp_keys`].
///
/// Blanket removal is used only for `[basecamp.env]` and a profile's `env`,
/// where `parse_string_map` rejects every key it cannot model and the parse
/// therefore *is* total.
fn write_config_into(mut doc: DocumentMut, cfg: &Config) -> DynResult<String> {
    // [scaffold]
    // Every `.entry(...).or_insert(...)` here goes through `coerce_to_table`
    // rather than `.as_table_mut().expect(...)`: `or_insert` hands back the
    // *occupied* item when the key already exists, and a root key the reader
    // tolerated as a non-table (an inline table, most plausibly) would
    // otherwise abort the process. See `coerce_to_table`.
    let scaffold = doc.entry("scaffold").or_insert(Item::Table(Table::new()));
    let scaffold_table = coerce_to_table(scaffold);
    scaffold_table["version"] = value(&cfg.version);
    if !cfg.cache_root.is_empty() {
        check_toml_value("cache_root", &cfg.cache_root)?;
        scaffold_table["cache_root"] = value(&cfg.cache_root);
    } else {
        remove_unless_empty_literal(scaffold_table, "cache_root");
    }

    // [repos.<name>] entries — render in stable order.
    write_repo_ref(&mut doc, "lez", &cfg.lez)?;
    write_repo_ref(&mut doc, "spel", &cfg.spel)?;
    // Both are `Option`, and the parse side reads a missing table back as
    // `None` — so dropping one from the config has to drop its table too, the
    // same way `[basecamp]` does below. No caller sets these to `None` today,
    // but `[basecamp]` had no such caller either right up until it did.
    for (name, repo) in [("basecamp", &cfg.basecamp_repo), ("lgpm", &cfg.lgpm_repo)] {
        match repo {
            Some(repo) => write_repo_ref(&mut doc, name, repo)?,
            None => {
                if let Some(repos) = doc.get_mut("repos").and_then(Item::as_table_mut) {
                    repos.remove(name);
                }
            }
        }
    }

    // [modules.<name>] entries. Drop any the config no longer carries: this
    // rewrite merges over the existing document, so a table left behind would
    // resurrect a module the caller removed.
    if let Some(modules) = doc.get_mut("modules").and_then(Item::as_table_mut) {
        modules.retain(|name, _| cfg.modules.contains_key(name));
        // A file written with an explicit `[modules]` header parses that table
        // as non-implicit, and toml_edit renders an empty non-implicit table
        // as a bare header — so emptying it is not enough to make it vanish.
        // Every entry under it is a `[modules.<name>]` table scaffold owns, so
        // there is no user content to strand here.
        if modules.is_empty() {
            doc.remove("modules");
        }
    }
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
        if let Some(app) = &entry.standalone_app {
            check_toml_value(&format!("modules.{name}.standalone_app"), app)?;
            table["standalone_app"] = value(app);
        } else {
            remove_unless_empty_literal(table, "standalone_app");
        }
        // Defensive: the function's check above already covered both fields.
        let _ = path;
    }

    // [wallet]
    check_toml_value("wallet.home_dir", &cfg.wallet_home_dir)?;
    let wallet = doc.entry("wallet").or_insert(Item::Table(Table::new()));
    coerce_to_table(wallet)["home_dir"] = value(&cfg.wallet_home_dir);

    // [framework] / [framework.idl]
    check_toml_value("framework.kind", &cfg.framework.kind)?;
    check_toml_value("framework.version", &cfg.framework.version)?;
    check_toml_value("framework.idl.spec", &cfg.framework.idl.spec)?;
    check_toml_value("framework.idl.path", &cfg.framework.idl.path)?;
    let framework = doc.entry("framework").or_insert(Item::Table(Table::new()));
    let framework_table = coerce_to_table(framework);
    framework_table["kind"] = value(&cfg.framework.kind);
    framework_table["version"] = value(&cfg.framework.version);
    let idl_table = child_table(framework_table, "idl");
    idl_table["spec"] = value(&cfg.framework.idl.spec);
    idl_table["path"] = value(&cfg.framework.idl.path);

    // [localnet]
    let localnet = doc.entry("localnet").or_insert(Item::Table(Table::new()));
    let localnet_table = coerce_to_table(localnet);
    localnet_table["port"] = value(i64::from(cfg.localnet.port));
    localnet_table["risc0_dev_mode"] = value(cfg.localnet.risc0_dev_mode);

    // [circuits]
    check_toml_value("circuits.version", &cfg.circuits.version)?;
    if let Some(template) = &cfg.circuits.url_template {
        check_toml_value("circuits.url_template", template)?;
        check_circuits_url_template(template)?;
    }
    check_toml_value("circuits.install_dir", &cfg.circuits.install_dir)?;
    let circuits = doc.entry("circuits").or_insert(Item::Table(Table::new()));
    let circuits_table = coerce_to_table(circuits);
    circuits_table["version"] = value(&cfg.circuits.version);
    if let Some(template) = &cfg.circuits.url_template {
        circuits_table["url_template"] = value(template);
    } else {
        remove_unless_empty_literal(circuits_table, "url_template");
    }
    if cfg.circuits.install_dir != CircuitsConfig::default().install_dir {
        circuits_table["install_dir"] = value(&cfg.circuits.install_dir);
    } else {
        remove_unless_empty_literal(circuits_table, "install_dir");
    }

    // [basecamp]
    if let Some(bc) = &cfg.basecamp {
        // Validate all string values up front, before borrowing `doc` mutably.
        for (k, v) in &bc.env {
            check_toml_value(&format!("basecamp.env.{k}"), v)?;
        }
        for (k, list) in &bc.env_append {
            for p in list {
                check_toml_value(&format!("basecamp.env_append.{k}"), p)?;
            }
        }
        for (profile, p) in &bc.profiles {
            // The profile name is itself a serialized table header, so guard it
            // like every other emitted name key (cf. `run.profiles.{name}`,
            // `modules.{name}`) — a control char in the key would corrupt the
            // line-oriented writer.
            check_toml_value(&format!("basecamp.profiles.{profile}"), profile)?;
            for (k, v) in &p.env {
                check_toml_value(&format!("basecamp.profiles.{profile}.env.{k}"), v)?;
            }
            if let Some(f) = &p.env_file {
                check_toml_value(&format!("basecamp.profiles.{profile}.env_file"), f)?;
            }
            if let Some(d) = &p.runtime_dir {
                check_toml_value(&format!("basecamp.profiles.{profile}.runtime_dir"), d)?;
            }
            if let Some(l) = &p.log_file {
                check_toml_value(&format!("basecamp.profiles.{profile}.log_file"), l)?;
            }
        }

        let basecamp = doc.entry("basecamp").or_insert(Item::Table(Table::new()));
        let basecamp_table = coerce_to_table(basecamp);
        // Only emit port keys when they differ from the defaults, so setting
        // just `[basecamp.env]` doesn't churn a user's scaffold.toml with
        // default `port_base`/`port_stride` on the next `save_project_config`.
        let default_bc = BasecampConfig::default();
        let mut wrote_direct_key = false;
        if bc.port_base != default_bc.port_base {
            basecamp_table["port_base"] = value(i64::from(bc.port_base));
            wrote_direct_key = true;
        } else {
            basecamp_table.remove("port_base");
        }
        if bc.port_stride != default_bc.port_stride {
            basecamp_table["port_stride"] = value(i64::from(bc.port_stride));
            wrote_direct_key = true;
        } else {
            basecamp_table.remove("port_stride");
        }
        // With no direct keys, an explicit `[basecamp]` header would render
        // empty — mark it implicit so only the child `[basecamp.env]` / etc.
        // tables appear. (Safe here precisely because there are no keys to get
        // dotted, which is the hazard the env subtables avoid via child_table.)
        if !wrote_direct_key {
            basecamp_table.set_implicit(true);
        }

        // Build the env subtables off `basecamp_table` directly. Routing
        // through `doc` via `ensure_subtable` would mark `[basecamp]` implicit
        // and render its real keys as dotted `basecamp.port_base = …` instead
        // of an explicit `[basecamp]` table.
        if !bc.env.is_empty() {
            let env_table = child_table(basecamp_table, "env");
            env_table.retain(|k, _| bc.env.contains_key(k));
            for (k, v) in &bc.env {
                env_table[k] = value(v);
            }
        } else {
            // Safe to drop wholesale: `parse_string_map` is total — it errors
            // on any key it cannot parse — so a loaded `[basecamp.env]`
            // contains nothing scaffold does not model.
            basecamp_table.remove("env");
        }
        if !bc.env_append.is_empty() {
            let append_table = child_table(basecamp_table, "env_append");
            // Unlike `[basecamp.env]`, this parse is *not* total: an empty
            // list is skipped rather than rejected (see the comment in
            // `parse_basecamp_runtime`), so the model's keys are a subset of
            // the file's. Keying the retain on the model alone would delete a
            // `FOO = []` the user wrote — so keep those too, and prune only
            // entries that carry a value the model no longer has.
            append_table.retain(|k, item| {
                bc.env_append.contains_key(k) || item.as_array().is_some_and(|a| a.is_empty())
            });
            for (k, list) in &bc.env_append {
                append_table[k] = string_array(list);
            }
        } else if let Some(append_table) = basecamp_table
            .get_mut("env_append")
            .and_then(Item::as_table_mut)
        {
            // Not a blanket remove, for the same reason as the retain above:
            // an empty list is skipped at parse, so a table holding *only*
            // those reads back as no `env_append` at all. Drop the entries the
            // model accounts for and keep the rest.
            append_table.retain(|_, item| item.as_array().is_some_and(|a| a.is_empty()));
            if append_table.is_empty() {
                basecamp_table.remove("env_append");
            }
        }
        if bc.profiles.is_empty() {
            // The branch below only prunes *within* an existing map, so
            // without this a profile cleared from the config while
            // `[basecamp]` itself survives would be left in the file and read
            // straight back on the next load. `parse_basecamp_runtime` drops a
            // fully-default profile, so clear only the modelled keys.
            if let Some(profiles) = basecamp_table
                .get_mut("profiles")
                .and_then(Item::as_table_mut)
            {
                for (_, item) in profiles.iter_mut() {
                    if let Some(profile) = item.as_table_mut() {
                        clear_owned_profile_keys(profile);
                    }
                }
                profiles.retain(|_, item| !item.as_table().is_some_and(Table::is_empty));
                if profiles.is_empty() {
                    basecamp_table.remove("profiles");
                }
            }
        }
        if !bc.profiles.is_empty() {
            let profiles = child_table(basecamp_table, "profiles");
            // Not total either: a fully-default profile is dropped at parse,
            // so a profile carrying only the user's own keys is absent from
            // the model. Prune by clearing what scaffold owns and keeping
            // whatever survives, rather than by the model's key set.
            profiles.retain(|name, item| {
                if bc.profiles.contains_key(name) {
                    return true;
                }
                match item.as_table_mut() {
                    Some(profile) => {
                        clear_owned_profile_keys(profile);
                        !profile.is_empty()
                    }
                    None => true,
                }
            });
            // Implicit so `[basecamp.profiles.<name>]` renders as the nested
            // header without an empty `[basecamp.profiles]` line.
            profiles.set_implicit(true);
            for (profile, p) in &bc.profiles {
                let profile_table = child_table(profiles, profile);
                // Scalar keys (env_file) render under the
                // `[basecamp.profiles.<name>]` header; the `env` child table
                // follows. With no scalar key, keep the profile table implicit
                // so only `[basecamp.profiles.<name>.env]` renders.
                let mut wrote_scalar = false;
                // The `else` arms keep an `env_file = ""` the user wrote:
                // `read_string` filters empty strings, so it reaches the model
                // as `None` — see `remove_unless_empty_literal`.
                if let Some(f) = &p.env_file {
                    profile_table["env_file"] = value(f);
                    wrote_scalar = true;
                } else {
                    remove_unless_empty_literal(profile_table, "env_file");
                    wrote_scalar |= profile_table.contains_key("env_file");
                }
                if let Some(d) = &p.runtime_dir {
                    profile_table["runtime_dir"] = value(d);
                    wrote_scalar = true;
                } else {
                    remove_unless_empty_literal(profile_table, "runtime_dir");
                    wrote_scalar |= profile_table.contains_key("runtime_dir");
                }
                if let Some(l) = &p.log_file {
                    profile_table["log_file"] = value(l);
                    wrote_scalar = true;
                } else {
                    remove_unless_empty_literal(profile_table, "log_file");
                    wrote_scalar |= profile_table.contains_key("log_file");
                }
                if !p.env.is_empty() {
                    let env_table = child_table(profile_table, "env");
                    env_table.retain(|k, _| p.env.contains_key(k));
                    for (k, v) in &p.env {
                        env_table[k] = value(v);
                    }
                } else {
                    profile_table.remove("env");
                }
                if !wrote_scalar {
                    profile_table.set_implicit(true);
                }
            }
        }
    }

    if cfg.basecamp.is_none() {
        clear_owned_basecamp_keys(&mut doc);
    }

    // [run] — only emit when non-default to keep fresh scaffold.toml minimal.
    write_run_config(&mut doc, &cfg.run)?;

    Ok(doc.to_string())
}

fn write_run_config(doc: &mut DocumentMut, run: &RunConfig) -> DynResult<()> {
    let has_inline = run.inline.reset
        || !run.inline.post_deploy.is_empty()
        || !run.inline.deploy
        || !run.inline.topup;
    let has_default_profile = run.default_profile.is_some();
    let has_profiles = !run.profiles.is_empty();
    let has_watch = run.watch != WatchConfig::default();
    // All-default: emit nothing, and drop any `[run]` the existing document
    // still carries — this writer merges over that document, so returning
    // early without removing would strand a section the config no longer has.
    if !has_inline && !has_default_profile && !has_profiles && !has_watch {
        if let Some(run_table) = doc.get_mut("run").and_then(Item::as_table_mut) {
            for k in ["reset", "deploy", "topup"] {
                run_table.remove(k);
            }
            // These two have an empty-literal form the reader skips.
            for k in ["default_profile", "post_deploy"] {
                remove_unless_empty_literal(run_table, k);
            }
            // `profiles` and `watch` are tables that may hold user keys of
            // their own, so recurse into them rather than removing outright.
            remove_owned_child(run_table, "watch", &["include", "exclude", "debounce_ms"]);
            prune_run_profiles(run_table);
            if run_table.is_empty() {
                doc.remove("run");
            }
        }
        return Ok(());
    }

    let run_item = doc.entry("run").or_insert(Item::Table(Table::new()));
    let run_table = coerce_to_table(run_item);
    if let Some(name) = &run.default_profile {
        check_toml_value("run.default_profile", name)?;
        run_table["default_profile"] = value(name);
    } else {
        remove_unless_empty_literal(run_table, "default_profile");
    }
    if run.inline.reset {
        run_table["reset"] = value(true);
    } else {
        run_table.remove("reset");
    }
    // Only emit `deploy`/`topup` when they deviate from the `true` default,
    // to keep a fresh scaffold.toml minimal.
    if !run.inline.deploy {
        run_table["deploy"] = value(false);
    } else {
        run_table.remove("deploy");
    }
    if !run.inline.topup {
        run_table["topup"] = value(false);
    } else {
        run_table.remove("topup");
    }
    if !run.inline.post_deploy.is_empty() {
        for hook in &run.inline.post_deploy {
            check_toml_value("run.post_deploy", hook)?;
        }
        run_table["post_deploy"] = post_deploy_value(&run.inline.post_deploy);
    } else {
        remove_unless_empty_literal(run_table, "post_deploy");
    }

    if has_profiles {
        // Drop profiles the config no longer carries before writing the rest.
        {
            let table = ensure_subtable(doc, "run", "profiles");
            table.retain(|name, _| run.profiles.contains_key(name));
        }
        for (name, profile) in &run.profiles {
            check_toml_value(&format!("run.profiles.{name}"), name)?;
            for hook in &profile.post_deploy {
                check_toml_value(&format!("run.profiles.{name}.post_deploy"), hook)?;
            }
            let table = ensure_subtable(doc, "run", "profiles");
            // ensure_subtable returns the `profiles` table; we need a
            // sub-sub-table keyed by `name`.
            table.set_implicit(true);
            let profile_table = child_table(table, name);
            if profile.reset {
                profile_table["reset"] = value(true);
            } else {
                profile_table.remove("reset");
            }
            if !profile.deploy {
                profile_table["deploy"] = value(false);
            } else {
                profile_table.remove("deploy");
            }
            if !profile.topup {
                profile_table["topup"] = value(false);
            } else {
                profile_table.remove("topup");
            }
            if !profile.post_deploy.is_empty() {
                profile_table["post_deploy"] = post_deploy_value(&profile.post_deploy);
            } else {
                remove_unless_empty_literal(profile_table, "post_deploy");
            }
        }
    } else if let Some(run_table) = doc.get_mut("run").and_then(Item::as_table_mut) {
        prune_run_profiles(run_table);
    }

    if has_watch {
        for g in run.watch.include.iter().chain(run.watch.exclude.iter()) {
            check_toml_value("run.watch", g)?;
        }
        let watch_table = ensure_subtable(doc, "run", "watch");
        if !run.watch.include.is_empty() {
            watch_table["include"] = string_array(&run.watch.include);
        } else {
            remove_unless_empty_literal(watch_table, "include");
        }
        if !run.watch.exclude.is_empty() {
            watch_table["exclude"] = string_array(&run.watch.exclude);
        } else {
            remove_unless_empty_literal(watch_table, "exclude");
        }
        if let Some(ms) = run.watch.debounce_ms {
            watch_table["debounce_ms"] = value(ms as i64);
        } else {
            watch_table.remove("debounce_ms");
        }
    } else if let Some(run_table) = doc.get_mut("run").and_then(Item::as_table_mut) {
        // `[run.watch]` parses to the default when it holds no modelled key,
        // so an all-unmodelled one is indistinguishable from absent — drop
        // only what we own, and the table only once it is empty.
        remove_owned_child(run_table, "watch", &["include", "exclude", "debounce_ms"]);
    }
    Ok(())
}

fn string_array(items: &[String]) -> Item {
    let mut arr = toml_edit::Array::new();
    for it in items {
        arr.push(it.as_str());
    }
    value(arr)
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
    for (system, a) in &repo.attr_platform {
        check_toml_value(&format!("repos.{name}.attr system key {system:?}"), system)?;
        check_toml_value(&format!("repos.{name}.attr.{system}"), a)?;
    }
    check_toml_value(&format!("repos.{name}.path"), &repo.path)?;
    let table = ensure_subtable(doc, "repos", name);
    table["source"] = value(&repo.source);
    table["pin"] = value(&repo.pin);
    // The `else` arms here go through `remove_unless_empty_literal` because
    // `parse_repo_ref` reads all three through `read_string`, which filters
    // empty strings — so `build = ""` / `attr = ""` / `path = ""` reach the
    // model as the same default a missing key gives.
    if repo.build != RepoBuild::default() {
        table["build"] = value(repo.build.as_str());
    } else {
        remove_unless_empty_literal(table, "build");
    }
    // Per-platform map wins over the scalar form; render it as an inline table
    // (`attr = { aarch64-darwin = "…" }`) so it stays a value under the
    // `[repos.<name>]` header rather than a dotted/child table.
    if !repo.attr_platform.is_empty() {
        let mut inline = toml_edit::InlineTable::new();
        for (system, a) in &repo.attr_platform {
            inline.insert(system, a.as_str().into());
        }
        table["attr"] = value(inline);
    } else if !repo.attr.is_empty() {
        table["attr"] = value(&repo.attr);
    } else {
        remove_unless_empty_literal(table, "attr");
    }
    if !repo.path.is_empty() {
        table["path"] = value(&repo.path);
    } else {
        remove_unless_empty_literal(table, "path");
    }
    Ok(())
}

/// Remove `key` unless the file holds it as an *empty literal* — `key = ""` or
/// `key = []`.
///
/// This is the third non-totality carve-out, alongside the two named on
/// [`write_config_into`], and it comes from the readers rather than from a
/// section parser. `read_string` ends in `.filter(|s| !s.is_empty())` and
/// `parse_post_deploy` / `parse_glob_list` map an empty string or array to an
/// empty `Vec`, so `env_file = ""`, `path = ""`, `post_deploy = ""` and
/// `exclude = []` all parse to exactly the model state that means "absent".
/// A plain `remove` on that state therefore deletes a line the user wrote —
/// and, because `toml_edit` hangs a comment off the following key's prefix
/// decor, the comment above it goes too.
///
/// Nothing observable changes either way: every consumer treats empty and
/// absent alike. But silently deleting a hand-written line and its comment is
/// the exact failure this rewrite exists to prevent, so preserve the literal
/// and let the user delete it themselves. Anything else — a stale non-empty
/// value the model no longer carries — is removed as usual.
fn remove_unless_empty_literal(table: &mut Table, key: &str) {
    let is_empty_literal = table.get(key).is_some_and(|item| {
        item.as_str().is_some_and(str::is_empty) || item.as_array().is_some_and(|a| a.is_empty())
    });
    if !is_empty_literal {
        table.remove(key);
    }
}

/// Clear the keys scaffold owns from one `[basecamp.profiles.<name>]`, keeping
/// any empty-string literal the reader skipped (see
/// [`remove_unless_empty_literal`]). Callers then drop the profile if it is
/// left empty.
fn clear_owned_profile_keys(profile: &mut Table) {
    for k in ["env_file", "runtime_dir", "log_file"] {
        remove_unless_empty_literal(profile, k);
    }
    // `env` is a table parsed by the total `parse_string_map`, so a blanket
    // remove is safe here — nothing in it is unmodelled.
    profile.remove("env");
}

/// Clear the keys scaffold owns from `table[key]`, then drop the child table
/// only if nothing is left in it.
///
/// A blanket `remove(key)` is wrong here. Scaffold's parsers return `None` (or
/// a default) for a section that carries no *modelled* field, so a section
/// holding only hand-written keys — a CI allocator's lease, a wrapper's own
/// setting, the comment explaining either — is indistinguishable from an
/// absent one at the model level. Removing it outright would delete user
/// content this rewrite exists to preserve. Emptiness after clearing what we
/// own is the only safe signal.
fn remove_owned_child(table: &mut Table, key: &str, owned: &[&str]) {
    let Some(child) = table.get_mut(key).and_then(Item::as_table_mut) else {
        return;
    };
    for k in owned {
        remove_unless_empty_literal(child, k);
    }
    if child.is_empty() {
        table.remove(key);
    }
}

/// Clear scaffold's keys from every `[run.profiles.<name>]`, dropping each
/// profile that is left empty and then `profiles` itself if none remain.
///
/// Unlike `parse_basecamp_runtime`, `parse_run` inserts every profile it sees
/// — it has no drop-if-default guard — so `run.profiles` and the file agree on
/// which names exist, and a model-keyed prune here would in fact be safe. This
/// is defensive rather than load-bearing: it keeps the two profile paths one
/// shape, so the `[basecamp]` analogue (where the guard *does* exist and a
/// model-keyed prune really does delete user content) cannot be reintroduced
/// here by someone copying this code. Do not restate the guard as `parse_run`
/// behaviour — it is not there.
fn prune_run_profiles(run_table: &mut Table) {
    let Some(profiles) = run_table.get_mut("profiles").and_then(Item::as_table_mut) else {
        return;
    };
    for (_, item) in profiles.iter_mut() {
        if let Some(profile) = item.as_table_mut() {
            for k in ["reset", "deploy", "topup"] {
                profile.remove(k);
            }
            remove_unless_empty_literal(profile, "post_deploy");
        }
    }
    profiles.retain(|_, item| !item.as_table().is_some_and(Table::is_empty));
    if profiles.is_empty() {
        run_table.remove("profiles");
    }
}

/// Clear everything scaffold owns from `[basecamp]` — the case where the whole
/// section parsed away to `None` — and drop the table only once nothing is
/// left. See [`remove_owned_child`] for why emptiness is the test rather than
/// the model being `None`.
///
/// The subtlety is that `[basecamp]`'s *children* carry the same carve-outs as
/// the section itself, and a blanket `remove("env_append")` /
/// `remove("profiles")` here would reach straight past them. When
/// `parse_basecamp_runtime` returns `None` it may be because every one of its
/// fields was individually skipped, not because the file holds nothing: an
/// `env_append` of only empty lists and a profile of only unmodelled keys both
/// parse away, and both are content the user wrote. So recurse with the same
/// scoped rules the non-`None` path uses rather than deleting the child tables
/// outright.
fn clear_owned_basecamp_keys(doc: &mut DocumentMut) {
    let Some(table) = doc.get_mut("basecamp").and_then(Item::as_table_mut) else {
        return;
    };
    table.remove("port_base");
    table.remove("port_stride");
    // Total parser (`parse_string_map`), so nothing unmodelled can be in here.
    table.remove("env");
    // Not total: an empty list is skipped at parse, so keep those.
    if let Some(append) = table.get_mut("env_append").and_then(Item::as_table_mut) {
        append.retain(|_, item| item.as_array().is_some_and(|a| a.is_empty()));
        if append.is_empty() {
            table.remove("env_append");
        }
    }
    // Not total: a fully-default profile is dropped at parse, so a profile of
    // only unmodelled keys is absent from the model but present in the file.
    if let Some(profiles) = table.get_mut("profiles").and_then(Item::as_table_mut) {
        for (_, item) in profiles.iter_mut() {
            if let Some(profile) = item.as_table_mut() {
                clear_owned_profile_keys(profile);
            }
        }
        profiles.retain(|_, item| !item.as_table().is_some_and(Table::is_empty));
        if profiles.is_empty() {
            table.remove("profiles");
        }
    }
    if table.is_empty() {
        doc.remove("basecamp");
    }
}

/// Coerce `item` into a real `Item::Table` in place, and hand back a mutable
/// borrow of it.
///
/// **Why this exists.** Every parser in this module reads its section through
/// [`Item::as_table`], which is `None` for an *inline* table
/// (`wallet = { home_dir = "…" }`) and for any non-table value. So a file
/// holding `wallet = { … }` parses perfectly well — the reader just falls back
/// to the default — and the writer then reached the same key expecting a
/// `Table`. `entry(...).or_insert(...)` returns the *occupied* item in that
/// case, not a freshly inserted table, so `.as_table_mut()` was `None` and the
/// old `.expect("wallet table")` aborted the process with a backtrace on a
/// `scaffold.toml` that is entirely valid TOML. `DOGFOODING.md` B1 calls a raw
/// panic on file content a UX regression, and weboko's review asked for a
/// returned error or an explicit fallback instead. This is the fallback, and it
/// is the data-preserving one:
///
/// - An **inline table** carries the user's own keys, so convert it rather than
///   discarding it: every key moves into a real `Table`, unmodelled ones
///   included. The section merely changes syntax — `wallet = { a = 1 }` becomes
///   `[wallet]\na = 1` — and the writer's own keys are then assigned over the
///   top exactly as they would have been. Nothing the user wrote is lost.
/// - A **non-table value** (`wallet = 3`, `wallet = "x"`) has no keys to keep
///   and cannot become a table without inventing structure. The parser already
///   ignored it, so the model never carried it and the config being written is
///   authoritative; replace it with an empty table. This is the "treat a
///   modelled key that isn't a table as unrepresentable" branch — the same
///   outcome the from-scratch fallback would give for that one section, but
///   scoped to it rather than to the whole file.
///
/// Either way the result is a `Table` and no path here can panic on file
/// content.
fn coerce_to_table(item: &mut Item) -> &mut Table {
    if item.as_table().is_none() {
        let replacement = match item.as_inline_table() {
            // `into_table` preserves every key, unmodelled ones included.
            Some(inline) => {
                let mut inline = inline.clone();
                // An inline table's decor is the whitespace *around the value*
                // on its `key = { … }` line. A real table's decor is the
                // whitespace around its `[header]`. Carrying the former into
                // the latter renders headers like "[\nframework.idl ]", which
                // is not the section the user wrote. The keys are what we are
                // preserving here; the punctuation around them is not worth
                // corrupting the file for, so reset it and let toml_edit lay
                // the promoted table out fresh.
                inline.decor_mut().clear();
                let mut table = inline.into_table();
                table.decor_mut().clear();
                table
            }
            None => Table::new(),
        };
        *item = Item::Table(replacement);
    }
    item.as_table_mut()
        .expect("coerce_to_table just installed an Item::Table")
}

/// Get or create a child `Table` under an existing `Table` without touching the
/// parent's implicit flag — unlike `ensure_subtable`, which marks its parent
/// implicit (wrong when the parent has real keys, e.g. `[basecamp]`).
///
/// Coerces rather than asserting, for the reasons in [`coerce_to_table`]: a
/// child key can be an inline table (`env = { FOO = "1" }`) just as easily as a
/// root one.
fn child_table<'a>(parent: &'a mut Table, name: &str) -> &'a mut Table {
    // A child written as `idl = { … }` carries decor on the *key* too, and for
    // a real table that decor is the whitespace inside the `[framework.idl]`
    // header. Carrying an inline key's decor across renders "[\nframework.idl ]"
    // — not the section the user wrote, and not even the same file. So reset
    // the key's decor alongside the item's own (which `coerce_to_table` does),
    // and only when we are actually about to promote.
    if !matches!(parent.get(name), Some(Item::Table(_))) {
        if let Some(mut key) = parent.key_mut(name) {
            key.leaf_decor_mut().clear();
            key.dotted_decor_mut().clear();
        }
    }
    coerce_to_table(parent.entry(name).or_insert(Item::Table(Table::new())))
}

fn ensure_subtable<'a>(doc: &'a mut DocumentMut, parent: &str, child: &str) -> &'a mut Table {
    let parent_item = doc.entry(parent).or_insert(Item::Table({
        let mut t = Table::new();
        t.set_implicit(true);
        t
    }));
    // `repos = { … }` / `modules = { … }` reach here the same way `[wallet]`
    // does — via a parser that reads them with `as_table` and shrugs at an
    // inline one. Coerce instead of asserting.
    let parent_table = coerce_to_table(parent_item);
    parent_table.set_implicit(true);
    child_table(parent_table, child)
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
        attr_platform: std::collections::BTreeMap::new(),
        path: String::new(),
    }
}

pub(crate) fn default_spel_repo(pin: &str) -> RepoRef {
    RepoRef {
        source: SPEL_SOURCE.to_string(),
        pin: pin.to_string(),
        build: RepoBuild::Cargo,
        attr: String::new(),
        attr_platform: std::collections::BTreeMap::new(),
        path: String::new(),
    }
}

pub(crate) fn default_basecamp_repo(pin: &str) -> RepoRef {
    RepoRef {
        source: BASECAMP_SOURCE.to_string(),
        pin: pin.to_string(),
        build: RepoBuild::NixFlake,
        attr: BASECAMP_ATTR.to_string(),
        attr_platform: std::collections::BTreeMap::new(),
        path: String::new(),
    }
}

pub(crate) fn default_lgpm_repo(pin: &str) -> RepoRef {
    RepoRef {
        source: LGPM_SOURCE.to_string(),
        pin: pin.to_string(),
        build: RepoBuild::NixFlake,
        attr: LGPM_ATTR.to_string(),
        attr_platform: std::collections::BTreeMap::new(),
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
    use crate::constants::{
        DEFAULT_BASECAMP_PIN, DEFAULT_CIRCUITS_VERSION, DEFAULT_LEZ, DEFAULT_LGPM_PIN, DEFAULT_SPEL,
    };

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
        assert_eq!(cfg.circuits.version, DEFAULT_CIRCUITS_VERSION);
        assert_eq!(cfg.circuits.install_dir, ".scaffold/circuits");
        assert_eq!(cfg.circuits.url_template, None);
    }

    #[test]
    fn parses_circuits_section() {
        let toml = minimal_v0_2_0()
            + r#"
[circuits]
version = "9.9.9"
url_template = "https://example.invalid/circuits-v{version}-{triple}.tar.gz"
install_dir = "vendor/circuits"
"#;
        let cfg = parse_config(&toml).expect("parse");
        assert_eq!(cfg.circuits.version, "9.9.9");
        assert_eq!(
            cfg.circuits.url_template.as_deref(),
            Some("https://example.invalid/circuits-v{version}-{triple}.tar.gz")
        );
        assert_eq!(cfg.circuits.install_dir, "vendor/circuits");
    }

    #[test]
    fn circuits_install_dir_rejects_parent_dir_traversal() {
        // `install_dir` is create_dir_all'd + extracted into; a `..` component
        // would escape the project root when joined.
        let toml = minimal_v0_2_0()
            + r#"
[circuits]
version = "9.9.9"
install_dir = "../../etc/evil"
"#;
        let err = parse_config(&toml).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("install_dir"), "{msg}");
        assert!(msg.contains(".."), "{msg}");
    }

    #[test]
    fn circuits_url_template_rejects_non_http_schemes() {
        for template in [
            "file:///tmp/circuits-{version}-{triple}.tar.gz",
            "ftp://example.invalid/circuits-{version}-{triple}.tar.gz",
            "example.invalid/circuits-{version}-{triple}.tar.gz",
        ] {
            let toml = minimal_v0_2_0()
                + &format!("[circuits]\nversion = \"9.9.9\"\nurl_template = {template:?}\n");
            let err = parse_config(&toml).unwrap_err();
            let msg = err.to_string();
            assert!(msg.contains("url_template"), "{msg}");
            assert!(msg.contains("http:// or https://"), "{msg}");
        }
    }

    #[test]
    fn circuits_url_template_accepts_http_and_https_case_insensitively() {
        for template in [
            "http://example.invalid/circuits-{version}-{triple}.tar.gz",
            "HTTPS://example.invalid/circuits-{version}-{triple}.tar.gz",
        ] {
            let toml = minimal_v0_2_0()
                + &format!("[circuits]\nversion = \"9.9.9\"\nurl_template = {template:?}\n");
            parse_config(&toml).expect("http(s) template should parse");
        }
    }

    #[test]
    fn circuits_section_requires_version_when_present() {
        let toml = minimal_v0_2_0() + "[circuits]\ninstall_dir = \"vendor/circuits\"\n";
        let err = parse_config(&toml).unwrap_err();
        assert!(err.to_string().contains("[circuits].version"), "{err}");
    }

    #[test]
    fn circuits_round_trips_through_serialize() {
        let toml = minimal_v0_2_0()
            + r#"
[circuits]
version = "9.9.9"
url_template = "https://example.invalid/circuits-v{version}-{triple}.tar.gz"
install_dir = "vendor/circuits"
"#;
        let cfg1 = parse_config(&toml).expect("parse");
        let serialized = serialize_config(&cfg1).expect("serialize");
        assert!(serialized.contains("[circuits]"), "{serialized}");
        assert!(serialized.contains("version = \"9.9.9\""), "{serialized}");
        assert!(
            serialized.contains("install_dir = \"vendor/circuits\""),
            "{serialized}"
        );
        let cfg2 = parse_config(&serialized).expect("re-parse");
        assert_eq!(cfg2.circuits.version, "9.9.9");
        assert_eq!(cfg2.circuits.install_dir, "vendor/circuits");
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
    fn repos_basecamp_attr_per_platform_map_parses_resolves_and_round_trips() {
        let toml = minimal_v0_2_0()
            + &format!(
                r#"
[repos.basecamp]
source = "{}"
pin = "{}"
build = "nix-flake"

[repos.basecamp.attr]
aarch64-darwin = "bin-macos-app"
x86_64-linux = "app"
"#,
                BASECAMP_SOURCE, DEFAULT_BASECAMP_PIN,
            );
        let cfg = parse_config(&toml).expect("parse");
        let bc = cfg.basecamp_repo.clone().expect("basecamp present");
        // Scalar `attr` stays empty for the table form; the map carries the values.
        assert!(bc.attr.is_empty());
        assert_eq!(bc.effective_attr("aarch64-darwin"), "bin-macos-app");
        assert_eq!(bc.effective_attr("x86_64-linux"), "app");
        // Unmapped platform falls back to the (empty) scalar.
        assert_eq!(bc.effective_attr("riscv64-linux"), "");

        // The per-platform map survives a serialize -> parse round-trip so
        // `save_project_config` (run by `setup`) never clobbers it.
        let serialized = serialize_config(&cfg).expect("serialize");
        let bc2 = parse_config(&serialized)
            .expect("re-parse")
            .basecamp_repo
            .expect("basecamp present after round-trip");
        assert_eq!(bc2.effective_attr("aarch64-darwin"), "bin-macos-app");
        assert_eq!(bc2.effective_attr("x86_64-linux"), "app");
        assert!(bc2.attr.is_empty());
    }

    #[test]
    fn repos_basecamp_attr_map_rejects_control_char_system_key() {
        // A quoted TOML key carrying a control char must be rejected at parse
        // so it can't corrupt the line-oriented serializer on the next save.
        let toml = minimal_v0_2_0()
            + &format!(
                "\n[repos.basecamp]\nsource = \"{}\"\npin = \"{}\"\nbuild = \"nix-flake\"\n",
                BASECAMP_SOURCE, DEFAULT_BASECAMP_PIN,
            )
            + "\n[repos.basecamp.attr]\n\"bad\\nkey\" = \"app\"\n";
        let err = parse_config(&toml).unwrap_err();
        assert!(err.to_string().contains("attr"), "{err}");
    }

    #[test]
    fn parses_basecamp_launch_env_sections() {
        let toml = minimal_v0_2_0()
            + r#"
[basecamp.env]
QT_DEBUG_PLUGINS = "1"

[basecamp.env_append]
QT_PLUGIN_PATH = ["/nix/store/a/plugins"]
LD_LIBRARY_PATH = ["/nix/store/a/lib", "/nix/store/b/lib"]

[basecamp.profiles.alice.env]
LOGOS_STORAGE_API_PORT = "8081"

[basecamp.profiles.bob.env]
LOGOS_STORAGE_API_PORT = "8082"
"#;
        let cfg = parse_config(&toml).expect("parse");
        let bc = cfg.basecamp.expect("basecamp config present");
        assert_eq!(
            bc.env.get("QT_DEBUG_PLUGINS").map(String::as_str),
            Some("1")
        );
        assert_eq!(
            bc.env_append.get("LD_LIBRARY_PATH").map(Vec::as_slice),
            Some(
                &[
                    "/nix/store/a/lib".to_string(),
                    "/nix/store/b/lib".to_string()
                ][..]
            )
        );
        assert_eq!(
            bc.profiles
                .get("alice")
                .and_then(|p| p.env.get("LOGOS_STORAGE_API_PORT"))
                .map(String::as_str),
            Some("8081")
        );
        assert_eq!(
            bc.profiles
                .get("bob")
                .and_then(|p| p.env.get("LOGOS_STORAGE_API_PORT"))
                .map(String::as_str),
            Some("8082")
        );
    }

    #[test]
    fn basecamp_env_append_drops_empty_lists() {
        // An empty list is a launch-time no-op; it must not be captured (so
        // `[basecamp]` stays empty here and nothing round-trips back).
        let toml = minimal_v0_2_0() + "[basecamp.env_append]\nQT_PLUGIN_PATH = []\n";
        let cfg = parse_config(&toml).expect("parse");
        assert!(
            cfg.basecamp.is_none(),
            "an empty env_append entry must not make [basecamp] non-empty: {:?}",
            cfg.basecamp
        );
    }

    #[test]
    fn basecamp_launch_env_round_trips_through_serialize() {
        let toml = minimal_v0_2_0()
            + r#"
[basecamp.env]
QT_DEBUG_PLUGINS = "1"

[basecamp.env_append]
QT_PLUGIN_PATH = ["/nix/store/a/plugins"]

[basecamp.profiles.alice.env]
LOGOS_STORAGE_API_PORT = "8081"
"#;
        let cfg1 = parse_config(&toml).expect("parse");
        let serialized = serialize_config(&cfg1).expect("serialize");
        let cfg2 = parse_config(&serialized).expect("re-parse");
        let bc = cfg2.basecamp.expect("basecamp present after round-trip");
        assert_eq!(
            bc.env.get("QT_DEBUG_PLUGINS").map(String::as_str),
            Some("1")
        );
        assert_eq!(
            bc.env_append.get("QT_PLUGIN_PATH").map(Vec::as_slice),
            Some(&["/nix/store/a/plugins".to_string()][..])
        );
        assert_eq!(
            bc.profiles
                .get("alice")
                .and_then(|p| p.env.get("LOGOS_STORAGE_API_PORT"))
                .map(String::as_str),
            Some("8081")
        );
    }

    #[test]
    fn basecamp_profile_scalars_and_custom_name_round_trip() {
        // A custom profile name (not alice/bob) carrying `env` plus all three
        // per-profile scalars parses, exposes them, and survives serialize ->
        // parse so `save_project_config` never drops them.
        let toml = minimal_v0_2_0()
            + r#"
[basecamp.profiles.carol]
env_file = ".scaffold/carol.env"
runtime_dir = "/tmp/lgs-carol"
log_file = ".scaffold/carol.log"

[basecamp.profiles.carol.env]
LOGOS_STORAGE_API_PORT = "8083"
"#;
        let assert_carol = |c: &BasecampProfile| {
            assert_eq!(c.env_file.as_deref(), Some(".scaffold/carol.env"));
            assert_eq!(c.runtime_dir.as_deref(), Some("/tmp/lgs-carol"));
            assert_eq!(c.log_file.as_deref(), Some(".scaffold/carol.log"));
            assert_eq!(
                c.env.get("LOGOS_STORAGE_API_PORT").map(String::as_str),
                Some("8083")
            );
        };
        let cfg = parse_config(&toml).expect("parse");
        assert_carol(
            cfg.basecamp
                .as_ref()
                .and_then(|bc| bc.profiles.get("carol"))
                .expect("carol profile"),
        );

        let serialized = serialize_config(&cfg).expect("serialize");
        let carol2 = parse_config(&serialized)
            .expect("re-parse")
            .basecamp
            .expect("basecamp present")
            .profiles
            .remove("carol")
            .expect("carol after round-trip");
        assert_carol(&carol2);
    }

    #[test]
    fn basecamp_env_only_omits_default_port_keys_and_avoids_dotting() {
        // Setting just [basecamp.env] (default ports) must NOT churn in
        // default port_base/port_stride, and must never serialize them as
        // dotted `basecamp.port_base = …` keys. Only [basecamp.env] renders.
        let toml = minimal_v0_2_0() + "[basecamp.env]\nQT_DEBUG_PLUGINS = \"1\"\n";
        let cfg = parse_config(&toml).expect("parse");
        let serialized = serialize_config(&cfg).expect("serialize");
        assert!(
            !serialized.contains("port_base"),
            "default port_base must be omitted (no churn), got:\n{serialized}"
        );
        assert!(
            serialized.contains("[basecamp.env]"),
            "expected [basecamp.env], got:\n{serialized}"
        );
        // Round-trips with env intact.
        let cfg2 = parse_config(&serialized).expect("re-parse");
        assert_eq!(
            cfg2.basecamp
                .and_then(|b| b.env.get("QT_DEBUG_PLUGINS").cloned())
                .as_deref(),
            Some("1")
        );
    }

    #[test]
    fn basecamp_non_default_ports_serialize_as_explicit_table() {
        // When a port differs from the default it is written under an explicit
        // [basecamp] header (not dotted), even alongside [basecamp.env].
        let toml = minimal_v0_2_0()
            + "[basecamp]\nport_base = 50000\n\n[basecamp.env]\nQT_DEBUG_PLUGINS = \"1\"\n";
        let cfg = parse_config(&toml).expect("parse");
        let serialized = serialize_config(&cfg).expect("serialize");
        assert!(
            serialized.contains("[basecamp]") && serialized.contains("port_base = 50000"),
            "expected explicit [basecamp] with port_base, got:\n{serialized}"
        );
        assert!(
            !serialized.contains("basecamp.port_base"),
            "port_base must not be a dotted key, got:\n{serialized}"
        );
        assert_eq!(
            parse_config(&serialized)
                .expect("re-parse")
                .basecamp
                .map(|b| b.port_base),
            Some(50000)
        );
    }

    #[test]
    fn basecamp_env_append_rejects_empty_string_entry() {
        // An empty path segment (`LD_LIBRARY_PATH=:`) silently injects CWD into
        // search paths — reject it at parse.
        let toml = minimal_v0_2_0() + "[basecamp.env_append]\nLD_LIBRARY_PATH = [\"\"]\n";
        let err = parse_config(&toml).unwrap_err();
        assert!(err.to_string().contains("must not be empty"), "{err}");
    }

    #[test]
    fn basecamp_env_rejects_invalid_var_name() {
        // `=` in an env var name would only surface as an opaque exec failure.
        let toml = minimal_v0_2_0() + "[basecamp.env]\n\"FOO=BAR\" = \"1\"\n";
        let err = parse_config(&toml).unwrap_err();
        assert!(err.to_string().contains("must not contain `=`"), "{err}");

        // Empty env var name is rejected too.
        let toml2 = minimal_v0_2_0() + "[basecamp.profiles.alice.env]\n\"\" = \"1\"\n";
        let err2 = parse_config(&toml2).unwrap_err();
        assert!(err2.to_string().contains("must not be empty"), "{err2}");
    }

    #[test]
    fn serialize_rejects_control_char_in_basecamp_profile_name() {
        // Profile names aren't validated at parse, so a quoted key with a
        // control char parses — but it must be rejected before it can corrupt
        // the serializer, like every other emitted name key.
        let toml = minimal_v0_2_0() + "[basecamp.profiles.\"bad\\nname\".env]\nFOO = \"1\"\n";
        let cfg = parse_config(&toml).expect("parse accepts the unchecked profile name");
        let err = serialize_config(&cfg).expect_err("serialize must reject the control-char name");
        assert!(
            err.to_string().contains("control character"),
            "expected control-char rejection, got: {err}"
        );
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
    fn module_standalone_app_parses_and_round_trips() {
        let toml = minimal_v0_2_0()
            + r#"
[modules.swap_ui]
flake = "path:./swap-ui#lgx"
role = "project"
standalone_app = "swap-ui-standalone"

[modules.swap]
flake = "path:./swap#lgx"
role = "project"
"#;
        let cfg = parse_config(toml.as_str()).expect("parse");
        assert_eq!(
            cfg.modules
                .get("swap_ui")
                .expect("swap_ui")
                .standalone_app
                .as_deref(),
            Some("swap-ui-standalone")
        );
        // A module that omits the field must stay `None` (not `Some("")`).
        assert_eq!(cfg.modules.get("swap").expect("swap").standalone_app, None);

        let serialized = serialize_config(&cfg).expect("serialize");
        let cfg2 = parse_config(&serialized).expect("re-parse");
        assert_eq!(
            cfg2.modules
                .get("swap_ui")
                .expect("swap_ui")
                .standalone_app
                .as_deref(),
            Some("swap-ui-standalone"),
            "standalone_app must survive serialize→parse so setup never clobbers it"
        );
        assert_eq!(cfg2.modules.get("swap").expect("swap").standalone_app, None);
        // An omitted/empty value must not be persisted as `standalone_app = ""`.
        assert!(
            !serialized.contains("standalone_app = \"\""),
            "empty standalone_app should be omitted: {serialized}"
        );
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
        assert!(err.to_string().contains("logos-scaffold init"), "{err}");
        let doc: DocumentMut = toml.parse().expect("re-parse for markers");
        let markers = detect_old_schema_markers(&doc, "0.2.0");
        assert!(markers.has_old_basecamp_keys, "{markers:?}");
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
        assert!(err.to_string().contains("logos-scaffold init"), "{err}");
        let doc: DocumentMut = toml.parse().expect("re-parse for markers");
        let markers = detect_old_schema_markers(&doc, "0.2.0");
        assert!(markers.has_old_basecamp_modules, "{markers:?}");
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

    /// `setup` rewrites scaffold.toml on every run. Project comments are
    /// routinely load-bearing — they record why a `runtime_dir` override or a
    /// held-back pin exists — so dropping them silently re-opens the bug they
    /// document.
    #[test]
    fn update_config_preserves_comments_and_unmodelled_keys() {
        let original = format!(
            "# why this project pins what it pins\n{}\n\
             # runtime_dir must be the session's real runtime dir: the 108-byte\n\
             # sun_path cap makes every module segfault otherwise.\n\
             [basecamp.profiles.alice]\n\
             runtime_dir = \"/run/user/1000\"\n",
            minimal_v0_2_0()
        );
        let cfg = parse_config(&original).expect("parse");
        let rewritten = update_config(&original, &cfg).expect("update");

        assert!(
            rewritten.contains("# why this project pins what it pins"),
            "leading comment lost:\n{rewritten}"
        );
        assert!(
            rewritten.contains("# sun_path cap makes every module segfault otherwise."),
            "load-bearing comment lost:\n{rewritten}"
        );
        assert!(
            rewritten.contains("/run/user/1000"),
            "the value the comment explains was lost:\n{rewritten}"
        );
        // And the result must still round-trip as valid config.
        parse_config(&rewritten).expect("rewritten config must parse");
    }

    /// Build a fixture where one modelled section is written as a root-level
    /// *inline* table instead of a `[header]`, dropping the header form so the
    /// inline one is the only definition of that key.
    fn with_inline_section(header: &str, inline: &str) -> String {
        let base = minimal_v0_2_0();
        let original = if header.is_empty() {
            format!("{inline}\n{base}")
        } else {
            assert!(
                base.contains(header),
                "minimal_v0_2_0 fixture drifted; no such block:\n{header}"
            );
            base.replace(header, &format!("{inline}\n"))
        };
        assert!(
            original.parse::<DocumentMut>().is_ok(),
            "fixture itself must be valid TOML:\n{original}"
        );
        original
    }

    /// Regression for weboko's review finding 1.
    ///
    /// `parse_config` reads every section through `Item::as_table`, which is
    /// `None` for an inline table — so `wallet = { home_dir = "…" }` is a
    /// perfectly valid `scaffold.toml` that parses fine and falls back to the
    /// default. The writer then reached the same key via
    /// `entry(...).or_insert(...)`, which returns the *occupied* item, and the
    /// old `.as_table_mut().expect("wallet table")` aborted the process with a
    /// backtrace. Every modelled root section was reachable that way; this
    /// covers each of them, plus the nested `ensure_subtable` /`child_table`
    /// paths (`repos`, `modules`, `run.profiles`, `framework.idl`,
    /// `basecamp.env`), which had the same `expect`.
    ///
    /// The contract asserted per case: the write must not panic, the result
    /// must still be valid TOML, it must still parse as a config, and the
    /// user's own keys inside the inline table must survive the coercion.
    #[test]
    fn update_config_survives_an_inline_table_for_every_modelled_section() {
        // (label, header block to drop, inline replacement, a key the user
        // wrote inside it that must survive).
        let cases = [
            (
                "wallet",
                "[wallet]\nhome_dir = \".scaffold/wallet\"\n",
                "wallet = { home_dir = \".scaffold/wallet\", note = \"keep me\" }",
                "keep me",
            ),
            (
                "framework",
                "[framework]\nkind = \"default\"\nversion = \"0.1.0\"\n",
                "framework = { kind = \"default\", version = \"0.1.0\", note = \"keep me\" }",
                "keep me",
            ),
            (
                "localnet",
                "[localnet]\nport = 3040\nrisc0_dev_mode = true\n",
                "localnet = { port = 3040, risc0_dev_mode = true, note = \"keep me\" }",
                "keep me",
            ),
            (
                "framework.idl",
                "[framework.idl]\nspec = \"lssa-idl/0.1.0\"\npath = \"idl\"\n",
                "idl = { spec = \"lssa-idl/0.1.0\", path = \"idl\", note = \"keep me\" }",
                "keep me",
            ),
            (
                "circuits",
                "",
                "circuits = { version = \"0.1.0\", note = \"keep me\" }",
                "keep me",
            ),
            (
                "run",
                "",
                "run = { reset = true, note = \"keep me\" }",
                "keep me",
            ),
            ("modules", "", "modules = { note = \"keep me\" }", "keep me"),
            (
                "basecamp",
                "",
                "basecamp = { port_base = 41000, note = \"keep me\" }",
                "keep me",
            ),
        ];

        for (label, header, inline, survivor) in cases {
            let original = with_inline_section(header, inline);
            // The whole point: this file is *valid* and the reader accepts it.
            let cfg = parse_config(&original)
                .unwrap_or_else(|e| panic!("{label}: fixture must parse, got {e}"));
            // Before the fix this aborted the process rather than returning.
            let rewritten = update_config(&original, &cfg)
                .unwrap_or_else(|e| panic!("{label}: update must not fail, got {e}"));
            assert!(
                rewritten.parse::<DocumentMut>().is_ok(),
                "{label}: rewrite is not valid TOML:\n{rewritten}"
            );
            parse_config(&rewritten)
                .unwrap_or_else(|e| panic!("{label}: rewrite must reparse, got {e}\n{rewritten}"));
            assert!(
                rewritten.contains(survivor),
                "{label}: the user's own key inside the inline table was dropped:\n{rewritten}"
            );
        }
    }

    /// `framework.idl` written inline *under* a real `[framework]` header —
    /// the `child_table` path rather than the root one.
    #[test]
    fn update_config_survives_an_inline_child_table() {
        let original = minimal_v0_2_0().replace(
            "[framework.idl]\nspec = \"lssa-idl/0.1.0\"\npath = \"idl\"\n",
            "",
        );
        let original = original.replace(
            "[framework]\nkind = \"default\"\nversion = \"0.1.0\"\n",
            "[framework]\nkind = \"default\"\nversion = \"0.1.0\"\n\
             idl = { spec = \"lssa-idl/0.1.0\", path = \"idl\", note = \"keep me\" }\n",
        );
        let cfg = parse_config(&original).expect("fixture must parse");
        let rewritten = update_config(&original, &cfg).expect("update must not panic");
        parse_config(&rewritten).expect("rewrite must reparse");
        assert!(
            rewritten.contains("keep me"),
            "unmodelled key inside the inline child table was dropped:\n{rewritten}"
        );
    }

    /// A modelled key that is not a table at all and has no keys to keep:
    /// there is nothing to coerce, so the section is replaced with the config's
    /// own rendering. It must not panic and must leave a parseable file.
    #[test]
    fn update_config_replaces_a_modelled_key_that_is_a_scalar() {
        for scalar in ["wallet = 3", "localnet = \"nope\"", "circuits = [1, 2]"] {
            let original = format!(
                "{scalar}\n{}",
                minimal_v0_2_0()
                    .replace("[wallet]\nhome_dir = \".scaffold/wallet\"\n", "")
                    .replace("[localnet]\nport = 3040\nrisc0_dev_mode = true\n", "")
            );
            let cfg = parse_config(&original)
                .unwrap_or_else(|e| panic!("{scalar}: fixture must parse, got {e}"));
            let rewritten = update_config(&original, &cfg)
                .unwrap_or_else(|e| panic!("{scalar}: update must not fail, got {e}"));
            parse_config(&rewritten)
                .unwrap_or_else(|e| panic!("{scalar}: rewrite must reparse, got {e}\n{rewritten}"));
        }
    }

    /// A hand-edit that broke the file must not block writing a valid one.
    #[test]
    fn update_config_falls_back_when_existing_is_unparseable() {
        let cfg = parse_config(&minimal_v0_2_0()).expect("parse");
        let rewritten = update_config("this is not valid toml", &cfg).expect("update");
        parse_config(&rewritten).expect("fallback output must parse");
        // The garbage must be replaced, not merged with: a fallback that
        // appended to the broken input would still parse and pass above.
        assert!(
            !rewritten.contains("this is not valid toml"),
            "unparseable input survived into the rewrite:\n{rewritten}"
        );
    }

    /// The dogfooding case: a comment explaining *why* a pin is what it is,
    /// sitting on a key the rewrite then reassigns. Leaving a key untouched
    /// and reassigning it are different `toml_edit` paths — decor survives the
    /// first trivially, so only this pins the second.
    #[test]
    fn update_config_keeps_a_comment_on_a_key_it_reassigns() {
        let original = minimal_v0_2_0().replace(
            "[repos.spel]",
            "# held back: 0.4 needs the new IDL spec\n[repos.spel]",
        );
        let mut cfg = parse_config(&original).expect("parse");
        cfg.spel.pin = "1111111111111111111111111111111111111111".to_string();
        let rewritten = update_config(&original, &cfg).expect("update");
        assert!(
            rewritten.contains("# held back: 0.4 needs the new IDL spec"),
            "comment lost when its section's key was reassigned:\n{rewritten}"
        );
        assert!(
            rewritten.contains("1111111111111111111111111111111111111111"),
            "new pin not written:\n{rewritten}"
        );
    }

    /// `--inspector` clears `attr_platform`, turning a `[repos.basecamp.attr]`
    /// header table into a scalar `attr = "…"`. That structural swap is the
    /// one the flag performs on every project carrying a per-platform map, so
    /// pin it rather than relying on `toml_edit` happening to do it right.
    #[test]
    fn update_config_replaces_a_per_platform_attr_table_with_a_scalar() {
        let original = minimal_v0_2_0()
            + "\n[repos.basecamp]\nsource = \"https://github.com/logos-co/logos-basecamp.git\"\n\
               pin = \"aa237766baf61404e12da86b7303cb41065464c9\"\nbuild = \"nix-flake\"\n\
               \n[repos.basecamp.attr]\nx86_64-linux = \"bin-appimage\"\n";
        let mut cfg = parse_config(&original).expect("parse");
        let bc = cfg.basecamp_repo.as_mut().expect("basecamp repo");
        bc.attr_platform.clear();
        bc.attr = "bin-bundle-dir-inspector".to_string();

        let rewritten = update_config(&original, &cfg).expect("update");
        let back = parse_config(&rewritten).expect("reparse");
        let rbc = back.basecamp_repo.as_ref().expect("basecamp repo");
        assert!(
            rbc.attr_platform.is_empty(),
            "stale per-platform map survived:\n{rewritten}"
        );
        assert_eq!(
            rbc.effective_attr("x86_64-linux"),
            "bin-bundle-dir-inspector",
            "map still wins over the scalar:\n{rewritten}"
        );
    }

    /// Rewriting an existing file must land the same values a fresh render
    /// would; only comments and untouched keys are extra.
    #[test]
    fn update_config_agrees_with_a_fresh_render_on_owned_keys() {
        let mut cfg = parse_config(&minimal_v0_2_0()).expect("parse");
        cfg.lez.path = "/abs/lez".to_string();
        let fresh = serialize_config(&cfg).expect("serialize");
        let updated = update_config(&minimal_v0_2_0(), &cfg).expect("update");
        assert_eq!(fresh, updated);
    }

    /// A scaffold.toml carrying every optional section, rewritten from a
    /// config whose values were all reset to their defaults.
    ///
    /// This is the case a naive in-place rewrite gets wrong. The writers below
    /// emit optional keys only when non-default (`if non_default { assign }`),
    /// which against a fresh document means "a default renders no key". Once
    /// the writer merges over the *existing* document, that same `if` stops
    /// removing anything: skipping the assignment leaves the old value in the
    /// file, so a value reset to its default silently persists and the config
    /// scaffold reads back is not the one it wrote. Every conditional emitter
    /// therefore needs an explicit removal branch, and comparing against a
    /// fresh render is what proves it has one.
    #[test]
    fn update_config_clears_keys_reset_to_defaults() {
        let original = minimal_v0_2_0()
            + "\n[basecamp]\nport_base = 41000\nport_stride = 7\n\
               \n[basecamp.env]\nFOO = \"bar\"\n\
               \n[basecamp.profiles.alice]\nruntime_dir = \"/run/user/1000\"\n\
               \n[modules.alpha]\nflake = \"./a#lgx\"\nrole = \"project\"\n\
               standalone_app = \"oldapp\"\n\
               \n[modules.beta]\nflake = \"./b#lgx\"\nrole = \"project\"\n\
               \n[run]\nreset = true\npost_deploy = \"echo hi\"\n\
               \n[circuits]\nversion = \"0.1.0\"\ninstall_dir = \"custom/dir\"\n";
        let mut cfg = parse_config(&original).expect("parse");

        // Reset everything optional back to its default / absent form.
        cfg.basecamp = None;
        cfg.modules.remove("beta");
        if let Some(alpha) = cfg.modules.get_mut("alpha") {
            alpha.standalone_app = None;
        }
        cfg.run = RunConfig::default();
        cfg.circuits.install_dir = CircuitsConfig::default().install_dir;

        let updated = update_config(&original, &cfg).expect("update");
        let fresh = serialize_config(&cfg).expect("serialize");
        // Compare the *set* of emitted lines, not the rendered string: an
        // in-place rewrite deliberately keeps each section where the user put
        // it, so section order legitimately differs from a fresh render.
        // Reordering someone's file would be as unwelcome as dropping their
        // comments; what must match is which keys exist and what they say.
        let lines = |s: &str| {
            let mut v: Vec<String> = s
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .map(str::to_string)
                .collect();
            v.sort();
            v
        };
        assert_eq!(
            lines(&fresh),
            lines(&updated),
            "in-place rewrite must not strand keys the config no longer carries\n\
             fresh:\n{fresh}\nupdated:\n{updated}"
        );

        // Spelled out, so a failure names the specific key that survived.
        let back = parse_config(&updated).expect("reparse");
        assert!(back.basecamp.is_none(), "[basecamp] survived:\n{updated}");
        assert!(
            !back.modules.contains_key("beta"),
            "removed module survived:\n{updated}"
        );
        assert_eq!(
            back.modules
                .get("alpha")
                .and_then(|m| m.standalone_app.as_deref()),
            None,
            "standalone_app survived:\n{updated}"
        );
        assert!(!back.run.inline.reset, "run.reset survived:\n{updated}");
        assert!(
            back.run.inline.post_deploy.is_empty(),
            "run.post_deploy survived:\n{updated}"
        );
        assert_eq!(
            back.circuits.install_dir,
            CircuitsConfig::default().install_dir,
            "circuits.install_dir survived:\n{updated}"
        );
    }

    /// The mirror image of the stale-key bug, and the worse failure: pruning
    /// must not reach past the keys scaffold owns. A key scaffold does not
    /// model, sitting inside a table it *does* own, is a hand-edit — a
    /// forward-compatible field, a note to a future reader — and deleting it
    /// would be a silent data loss no comment could survive.
    #[test]
    fn update_config_keeps_unmodelled_keys_inside_tables_it_owns() {
        let original = minimal_v0_2_0().replace(
            "[repos.spel]",
            "[repos.spel]\nfuture_field = \"scaffold does not model this\"",
        ) + "\n[some_unknown_section]\nkey = \"value\"\n";
        let mut cfg = parse_config(&original).expect("parse");
        // Touch a key scaffold *does* own in that same table.
        cfg.spel.pin = "3333333333333333333333333333333333333333".to_string();

        let rewritten = update_config(&original, &cfg).expect("update");
        assert!(
            rewritten.contains("future_field = \"scaffold does not model this\""),
            "unmodelled key inside a scaffold-owned table was deleted:\n{rewritten}"
        );
        assert!(
            rewritten.contains("[some_unknown_section]"),
            "a section scaffold does not own was deleted:\n{rewritten}"
        );
        assert!(
            rewritten.contains("3333333333333333333333333333333333333333"),
            "the edit itself was not written:\n{rewritten}"
        );
    }

    /// A `[repos.basecamp]` dropped from the config must lose its table, the
    /// same as `[basecamp]`. The parse side reads a missing table back as
    /// `None`, so a stranded one silently resurrects the repo.
    #[test]
    fn update_config_drops_a_repo_table_the_config_no_longer_carries() {
        let original = minimal_v0_2_0()
            + "\n[repos.basecamp]\nsource = \"https://github.com/logos-co/logos-basecamp.git\"\n\
               pin = \"aa237766baf61404e12da86b7303cb41065464c9\"\nbuild = \"nix-flake\"\n\
               attr = \"app\"\n";
        let mut cfg = parse_config(&original).expect("parse");
        assert!(cfg.basecamp_repo.is_some(), "fixture must start with one");
        cfg.basecamp_repo = None;

        let rewritten = update_config(&original, &cfg).expect("update");
        assert!(
            !rewritten.contains("[repos.basecamp]"),
            "dropped repo table survived:\n{rewritten}"
        );
        assert!(
            parse_config(&rewritten)
                .expect("reparse")
                .basecamp_repo
                .is_none(),
            "dropped repo reparsed as present:\n{rewritten}"
        );
        // The sibling repos must be untouched.
        assert!(rewritten.contains("[repos.lez]"), "{rewritten}");
        assert!(rewritten.contains("[repos.spel]"), "{rewritten}");
    }

    /// A file carrying only `[basecamp.env]` parses `[basecamp]` as implicit.
    /// Assigning a non-default `port_base` into a table still flagged implicit
    /// would render it as a dotted `basecamp.port_base = …` under whatever
    /// section came before — landing the key in the wrong place entirely.
    #[test]
    fn update_config_promotes_an_implicit_basecamp_table_when_a_direct_key_appears() {
        let original = minimal_v0_2_0() + "\n[basecamp.env]\nFOO = \"bar\"\n";
        let mut cfg = parse_config(&original).expect("parse");
        cfg.basecamp.as_mut().expect("basecamp").port_base = 41000;

        let rewritten = update_config(&original, &cfg).expect("update");
        let back = parse_config(&rewritten).expect("reparse");
        assert_eq!(
            back.basecamp.as_ref().expect("basecamp").port_base,
            41000,
            "port_base did not survive the round trip:\n{rewritten}"
        );
        assert!(
            !rewritten.contains("basecamp.port_base"),
            "key rendered dotted instead of under a [basecamp] header:\n{rewritten}"
        );
    }

    /// A section scaffold models but that currently holds *only* unmodelled
    /// user keys parses to `None` — there is no modelled field to see — so a
    /// blanket `doc.remove(...)` for the `None` case deletes the user's
    /// content. Removing a section is only safe once the keys scaffold owns
    /// are gone and nothing else is left.
    #[test]
    fn update_config_keeps_a_section_holding_only_unmodelled_keys() {
        let original = minimal_v0_2_0()
            + "\n# ports come from the CI allocator; do not hand-edit\n\
               [basecamp]\nci_port_lease = \"team-alpha\"\n\
               \n[run]\nmakefile_target = \"deploy\"\n\
               \n[run.watch]\npoll_interval_ms = 250\n";
        let cfg = parse_config(&original).expect("parse");
        // Nothing modelled in either section, so both parse away entirely.
        assert!(cfg.basecamp.is_none(), "fixture assumption");

        let rewritten = update_config(&original, &cfg).expect("update");
        assert!(
            rewritten.contains("ci_port_lease = \"team-alpha\""),
            "[basecamp] holding only user keys was deleted:\n{rewritten}"
        );
        assert!(
            rewritten.contains("# ports come from the CI allocator; do not hand-edit"),
            "the comment explaining it went with it:\n{rewritten}"
        );
        assert!(
            rewritten.contains("makefile_target = \"deploy\""),
            "[run] holding only user keys was deleted:\n{rewritten}"
        );
        assert!(
            rewritten.contains("poll_interval_ms = 250"),
            "[run.watch] holding only user keys was deleted:\n{rewritten}"
        );
    }

    /// The two requirements have to hold at once, in the same table: scaffold
    /// clears the keys it owns, and leaves everything else exactly as written.
    /// Testing either alone would pass with the other broken — a writer that
    /// removed nothing preserves user keys trivially, and one that removed the
    /// whole table clears scaffold's keys trivially.
    #[test]
    fn update_config_clears_owned_keys_while_keeping_user_keys_in_one_table() {
        let original = minimal_v0_2_0()
            + "\n[basecamp]\nport_base = 41000\nci_port_lease = \"team-alpha\"\n\
               \n[run]\nreset = true\nmakefile_target = \"deploy\"\n\
               \n[run.watch]\ninclude = [\"src/**\"]\npoll_interval_ms = 250\n";
        let mut cfg = parse_config(&original).expect("parse");
        // Reset every modelled value in those tables to its default.
        cfg.basecamp.as_mut().expect("basecamp").port_base = BasecampConfig::default().port_base;
        cfg.run = RunConfig::default();

        let rewritten = update_config(&original, &cfg).expect("update");

        // Scaffold's keys are gone. Assert on the reparsed model rather than
        // substrings: `!contains("include")` would also match a comment or a
        // longer key, and — worse — it passes just as happily if the whole
        // table was deleted, which is the failure the second half exists to
        // catch.
        // (`[basecamp]` now reparses to `None`: only the user's unmodelled key
        // is left in it, and that is exactly the state the second half checks
        // is still *present in the file*.)
        let back = parse_config(&rewritten).expect("rewritten config must still parse");
        assert!(
            back.basecamp.is_none(),
            "owned key survived a reset to default:\n{rewritten}"
        );
        assert!(
            !back.run.inline.reset,
            "owned key survived a reset to default:\n{rewritten}"
        );
        assert!(
            back.run.watch.include.is_empty(),
            "owned key survived a reset to default:\n{rewritten}"
        );
        // Pinned as full assignments so a stray substring cannot satisfy them.
        assert!(
            !rewritten.contains("port_base ="),
            "owned key survived a reset to default:\n{rewritten}"
        );
        assert!(
            !rewritten.contains("include ="),
            "owned key survived a reset to default:\n{rewritten}"
        );
        // ...and the user's are untouched, tables and all.
        assert!(
            rewritten.contains("ci_port_lease = \"team-alpha\""),
            "user key deleted alongside an owned one:\n{rewritten}"
        );
        assert!(
            rewritten.contains("makefile_target = \"deploy\""),
            "user key deleted alongside an owned one:\n{rewritten}"
        );
        assert!(
            rewritten.contains("poll_interval_ms = 250"),
            "user key deleted alongside an owned one:\n{rewritten}"
        );
        parse_config(&rewritten).expect("rewritten config must still parse");
    }

    /// `parse_basecamp_runtime` *skips* an empty `env_append` list instead of
    /// erroring on it, so unlike `[basecamp.env]` the parse is not total and
    /// the model's key set is narrower than the file's. A `retain` keyed on
    /// the model therefore deletes a line the user wrote.
    #[test]
    fn update_config_keeps_an_empty_env_append_list() {
        let original = minimal_v0_2_0()
            + "\n[basecamp.env_append]\n\
               # cleared deliberately; the wrapper appends to it at runtime\n\
               LD_LIBRARY_PATH = []\nPATH = [\"/opt/bin\"]\n";
        let cfg = parse_config(&original).expect("parse");
        assert!(
            !cfg.basecamp
                .as_ref()
                .expect("basecamp")
                .env_append
                .contains_key("LD_LIBRARY_PATH"),
            "fixture assumption: the empty list is skipped by the parser"
        );

        let rewritten = update_config(&original, &cfg).expect("update");
        assert!(
            rewritten.contains("LD_LIBRARY_PATH"),
            "an empty env_append list the parser skipped was deleted:\n{rewritten}"
        );
        assert!(
            rewritten.contains("PATH = [\"/opt/bin\"]"),
            "the modelled sibling was lost:\n{rewritten}"
        );

        // The retain above only runs while the model is non-empty. Drop the
        // modelled sibling and the whole table takes the `else` path, which
        // must still not delete what the parser merely skipped.
        let only_empty = minimal_v0_2_0()
            + "\n[basecamp]\nport_base = 41000\n\
               \n[basecamp.env_append]\nLD_LIBRARY_PATH = []\n";
        let cfg = parse_config(&only_empty).expect("parse");
        assert!(
            cfg.basecamp
                .as_ref()
                .expect("basecamp")
                .env_append
                .is_empty(),
            "fixture assumption: the model sees no env_append at all"
        );
        let rewritten = update_config(&only_empty, &cfg).expect("update");
        assert!(
            rewritten.contains("LD_LIBRARY_PATH"),
            "the sole skipped entry was deleted with its table:\n{rewritten}"
        );
    }

    /// Same asymmetry one level over: `parse_basecamp_runtime` drops a
    /// fully-default profile, so a `[basecamp.profiles.<name>]` holding only
    /// unmodelled keys is absent from the model but present in the file.
    #[test]
    fn update_config_keeps_a_profile_holding_only_unmodelled_keys() {
        let original = minimal_v0_2_0()
            + "\n[basecamp.profiles.alice]\nruntime_dir = \"/run/user/1000\"\n\
               \n[basecamp.profiles.carol]\n\
               # our harness reads this; scaffold does not model it\n\
               screenshot_dir = \"/tmp/shots\"\n";
        let cfg = parse_config(&original).expect("parse");
        assert!(
            !cfg.basecamp
                .as_ref()
                .expect("basecamp")
                .profiles
                .contains_key("carol"),
            "fixture assumption: an all-unmodelled profile is skipped"
        );

        let rewritten = update_config(&original, &cfg).expect("update");
        assert!(
            rewritten.contains("screenshot_dir = \"/tmp/shots\""),
            "a profile the parser skipped was deleted:\n{rewritten}"
        );
        assert!(
            rewritten.contains("/run/user/1000"),
            "the modelled sibling profile was lost:\n{rewritten}"
        );
    }

    /// Clearing the last profile while `[basecamp]` itself survives must not
    /// leave the profile in the file — the prune below only walks entries
    /// *within* a non-empty map, so this needs its own branch.
    #[test]
    fn update_config_drops_a_cleared_basecamp_profile() {
        let original = minimal_v0_2_0()
            + "\n[basecamp]\nport_base = 41000\n\
               \n[basecamp.profiles.alice]\nruntime_dir = \"/run/user/1000\"\n";
        let mut cfg = parse_config(&original).expect("parse");
        cfg.basecamp.as_mut().expect("basecamp").profiles.clear();

        let rewritten = update_config(&original, &cfg).expect("update");
        assert!(
            !rewritten.contains("[basecamp.profiles.alice]"),
            "cleared profile survived:\n{rewritten}"
        );
        let back = parse_config(&rewritten).expect("reparse");
        assert!(
            back.basecamp
                .as_ref()
                .expect("basecamp")
                .profiles
                .is_empty(),
            "cleared profile reparsed as present:\n{rewritten}"
        );
        assert_eq!(
            back.basecamp.as_ref().expect("basecamp").port_base,
            41000,
            "the surviving parent lost its own key:\n{rewritten}"
        );
    }

    /// A file written with an explicit `[modules]` header parses that table as
    /// non-implicit, and toml_edit renders an empty non-implicit table as a
    /// bare header — so emptying it is not enough to make it disappear.
    #[test]
    fn update_config_drops_an_explicit_modules_header_when_emptied() {
        let original = minimal_v0_2_0()
            + "\n[modules]\n\n[modules.alpha]\nflake = \"./a#lgx\"\nrole = \"project\"\n";
        let mut cfg = parse_config(&original).expect("parse");
        cfg.modules.clear();

        let rewritten = update_config(&original, &cfg).expect("update");
        assert!(
            !rewritten.contains("[modules"),
            "bare [modules] header stranded:\n{rewritten}"
        );
    }

    /// Removing the *last* module must not strand a bare `[modules]` header.
    /// `retain` empties the table rather than deleting it, and an empty
    /// explicit table still renders its header — which then reparses as an
    /// empty section rather than no section.
    #[test]
    fn update_config_drops_the_modules_table_when_the_last_entry_goes() {
        let original =
            minimal_v0_2_0() + "\n[modules.alpha]\nflake = \"./a#lgx\"\nrole = \"project\"\n";
        let mut cfg = parse_config(&original).expect("parse");
        cfg.modules.clear();

        let updated = update_config(&original, &cfg).expect("update");
        assert!(
            !updated.contains("[modules"),
            "empty modules table left behind:\n{updated}"
        );
        assert_eq!(
            updated,
            serialize_config(&cfg).expect("serialize"),
            "rewrite diverged from a fresh render:\n{updated}"
        );
    }

    /// Same edge one level down, where the parent carries real keys and is
    /// therefore *not* implicit: clearing every `[basecamp.env]` entry while
    /// `[basecamp]` itself survives must leave no empty child header.
    #[test]
    fn update_config_drops_an_emptied_basecamp_env_but_keeps_its_parent() {
        let original =
            minimal_v0_2_0() + "\n[basecamp]\nport_base = 41000\n\n[basecamp.env]\nFOO = \"bar\"\n";
        let mut cfg = parse_config(&original).expect("parse");
        cfg.basecamp.as_mut().expect("basecamp").env.clear();

        let updated = update_config(&original, &cfg).expect("update");
        assert!(
            !updated.contains("[basecamp.env]"),
            "empty env table left behind:\n{updated}"
        );
        // The non-default port must survive — only the emptied child goes.
        let back = parse_config(&updated).expect("reparse");
        assert_eq!(
            back.basecamp.as_ref().expect("basecamp").port_base,
            41000,
            "clearing a child table clobbered its parent's keys:\n{updated}"
        );
    }

    /// The `[basecamp] = None` path has to honour the *children's* carve-outs,
    /// not just the section's own. `parse_basecamp_runtime` can return `None`
    /// because every field was individually skipped rather than because the
    /// file is empty — an `env_append` of only empty lists and a profile of
    /// only unmodelled keys both parse away — so clearing the section by
    /// blanket-removing `env_append` and `profiles` deletes content the user
    /// wrote. The non-`None` path already scopes these; this pins that the
    /// `None` path does too.
    #[test]
    fn update_config_keeps_skipped_basecamp_children_when_the_section_parses_to_none() {
        let original = minimal_v0_2_0()
            + "\n# ports come from the CI allocator\n[basecamp]\nci_lease = \"team-a\"\n\
               \n[basecamp.env_append]\n\
               # cleared deliberately; the wrapper appends at runtime\n\
               LD_LIBRARY_PATH = []\n\
               \n[basecamp.profiles.carol]\n\
               # our harness reads this; scaffold does not model it\n\
               screenshot_dir = \"/tmp/shots\"\n";
        let cfg = parse_config(&original).expect("parse");
        assert!(
            cfg.basecamp.is_none(),
            "fixture assumption: every field is skipped, so the section is None"
        );

        let rewritten = update_config(&original, &cfg).expect("update");
        assert!(
            rewritten.contains("ci_lease = \"team-a\""),
            "the section's own unmodelled key was deleted:\n{rewritten}"
        );
        assert!(
            rewritten.contains("LD_LIBRARY_PATH = []"),
            "an env_append entry the parser merely skipped was deleted:\n{rewritten}"
        );
        assert!(
            rewritten.contains("# cleared deliberately; the wrapper appends at runtime"),
            "the comment explaining it went with it:\n{rewritten}"
        );
        assert!(
            rewritten.contains("screenshot_dir = \"/tmp/shots\""),
            "a profile the parser merely skipped was deleted:\n{rewritten}"
        );
        assert!(
            rewritten.contains("# our harness reads this; scaffold does not model it"),
            "the comment explaining the profile went with it:\n{rewritten}"
        );
        parse_config(&rewritten).expect("rewritten config must still parse");
    }

    /// `read_string` ends in `.filter(|s| !s.is_empty())`, and
    /// `parse_post_deploy` / `parse_glob_list` map an empty string or array to
    /// an empty `Vec` — so `path = ""`, `post_deploy = ""` and `exclude = []`
    /// all reach the model as exactly the state that means "key absent". A
    /// plain `remove` on that state deletes a line the user wrote, and takes
    /// the comment above it with it. Nothing observable changes either way,
    /// but silent deletion of a hand-written line is the failure this rewrite
    /// exists to prevent.
    #[test]
    fn update_config_keeps_empty_literals_the_readers_skip() {
        let original = minimal_v0_2_0().replace(
            "[repos.spel]",
            "[repos.spel]\n# resolved at runtime; deliberately blank\npath = \"\"",
        ) + "\n[basecamp]\nport_base = 41000\n\
               \n[basecamp.profiles.alice]\nruntime_dir = \"/run/user/1000\"\n\
               # the wrapper supplies this\nenv_file = \"\"\n\
               \n[run]\nreset = true\n\
               \n[run.watch]\ninclude = [\"src/**\"]\n\
               # nothing to exclude yet\nexclude = []\n";
        let cfg = parse_config(&original).expect("parse");
        // Fixture assumption: each of these reaches the model as "absent".
        assert!(cfg.spel.path.is_empty(), "fixture: path parses away");
        assert!(
            cfg.basecamp
                .as_ref()
                .expect("basecamp")
                .profiles
                .get("alice")
                .expect("alice")
                .env_file
                .is_none(),
            "fixture: empty env_file parses away"
        );
        assert!(
            cfg.run.watch.exclude.is_empty(),
            "fixture: exclude is empty"
        );

        let rewritten = update_config(&original, &cfg).expect("update");
        assert!(
            rewritten.contains("path = \"\""),
            "an empty `path` literal was deleted:\n{rewritten}"
        );
        assert!(
            rewritten.contains("# resolved at runtime; deliberately blank"),
            "the comment above it went with it:\n{rewritten}"
        );
        assert!(
            rewritten.contains("env_file = \"\""),
            "an empty `env_file` literal was deleted:\n{rewritten}"
        );
        assert!(
            rewritten.contains("# the wrapper supplies this"),
            "the comment above env_file went with it:\n{rewritten}"
        );
        assert!(
            rewritten.contains("exclude = []"),
            "an empty `exclude` array was deleted:\n{rewritten}"
        );
        assert!(
            rewritten.contains("# nothing to exclude yet"),
            "the comment above exclude went with it:\n{rewritten}"
        );
        // Preserving them must not change what the file means.
        let back = parse_config(&rewritten).expect("reparse");
        assert!(back.spel.path.is_empty(), "empty literal changed meaning");
        assert!(
            back.run.watch.exclude.is_empty(),
            "empty literal changed meaning"
        );
    }

    /// A stale *non-empty* value is still removed — the empty-literal carve-out
    /// above must not turn into "never remove anything". This is the class-A
    /// half of the same helper, and it is what stops the fix for one failure
    /// mode from reintroducing the other.
    #[test]
    fn update_config_still_clears_non_empty_optional_values() {
        let original = minimal_v0_2_0().replace(
            "[scaffold]\nversion = \"0.2.0\"",
            "[scaffold]\nversion = \"0.2.0\"\ncache_root = \"/custom/cache\"",
        ) + "\n[circuits]\nversion = \"0.1.0\"\n\
               url_template = \"https://example.com/{version}.tar.gz\"\n\
               \n[run]\ndefault_profile = \"dev\"\npost_deploy = \"echo hi\"\n\
               \n[run.profiles.dev]\nreset = true\npost_deploy = \"echo dev\"\n\
               \n[run.profiles.ci]\ndeploy = false\n\
               \n[run.watch]\ninclude = [\"src/**\"]\ndebounce_ms = 500\n";
        let mut cfg = parse_config(&original).expect("parse");
        assert!(!cfg.cache_root.is_empty(), "fixture: cache_root is set");
        assert!(cfg.circuits.url_template.is_some(), "fixture: template set");

        // Reset each optional back to its default / absent form, while keeping
        // enough non-default state that the writer takes the *emitting* path
        // rather than the whole-section early return.
        cfg.cache_root.clear();
        cfg.circuits.url_template = None;
        cfg.run.default_profile = None;
        cfg.run.inline.post_deploy.clear();
        let dev = cfg.run.profiles.get_mut("dev").expect("dev profile");
        dev.reset = false;
        dev.post_deploy.clear();
        cfg.run.watch.include.clear();

        let rewritten = update_config(&original, &cfg).expect("update");
        let back = parse_config(&rewritten).expect("reparse");
        assert!(
            back.cache_root.is_empty(),
            "stale cache_root survived:\n{rewritten}"
        );
        assert!(
            back.circuits.url_template.is_none(),
            "stale url_template survived:\n{rewritten}"
        );
        assert!(
            back.run.default_profile.is_none(),
            "stale default_profile survived:\n{rewritten}"
        );
        assert!(
            back.run.inline.post_deploy.is_empty(),
            "stale run.post_deploy survived:\n{rewritten}"
        );
        let back_dev = back.run.profiles.get("dev").expect("dev survives");
        assert!(
            !back_dev.reset,
            "stale profile reset survived:\n{rewritten}"
        );
        assert!(
            back_dev.post_deploy.is_empty(),
            "stale profile post_deploy survived:\n{rewritten}"
        );
        assert!(
            back.run.watch.include.is_empty(),
            "stale watch include survived:\n{rewritten}"
        );
        // The keys that were *not* reset must be untouched, so this cannot
        // pass by deleting whole sections.
        assert_eq!(back.run.watch.debounce_ms, Some(500), "{rewritten}");
        assert!(
            !back.run.profiles.get("ci").expect("ci survives").deploy,
            "an untouched sibling profile was clobbered:\n{rewritten}"
        );
    }

    /// `write_repo_ref`'s three optionals, cleared under a merge. The fresh
    /// render never exercised the removal side, and `[repos.<name>].path` is
    /// the operationally sharp one: a stranded absolute path pins the project
    /// to one machine's layout.
    #[test]
    fn update_config_clears_repo_optionals_reset_to_defaults() {
        let original = minimal_v0_2_0()
            + "\n[repos.basecamp]\nsource = \"https://github.com/logos-co/logos-basecamp.git\"\n\
               pin = \"aa237766baf61404e12da86b7303cb41065464c9\"\n\
               build = \"nix-flake\"\nattr = \"app\"\npath = \"/abs/basecamp\"\n";
        let mut cfg = parse_config(&original).expect("parse");
        let bc = cfg.basecamp_repo.as_mut().expect("basecamp repo");
        assert_ne!(bc.build, RepoBuild::default(), "fixture: build non-default");
        bc.build = RepoBuild::default();
        bc.attr.clear();
        bc.path.clear();

        let rewritten = update_config(&original, &cfg).expect("update");
        let back = parse_config(&rewritten).expect("reparse");
        let rbc = back.basecamp_repo.as_ref().expect("repo survives");
        assert_eq!(rbc.build, RepoBuild::default(), "stale build:\n{rewritten}");
        assert!(rbc.attr.is_empty(), "stale attr:\n{rewritten}");
        assert!(rbc.path.is_empty(), "stale path:\n{rewritten}");
        // The repo itself must survive — this must not pass by deleting it.
        assert_eq!(rbc.pin, "aa237766baf61404e12da86b7303cb41065464c9");
    }

    /// `[repos.lgpm]` takes the same `None` arm as `[repos.basecamp]`, and
    /// `setup` writes both. Only `basecamp` was pinned; a loop is easy to
    /// half-break.
    #[test]
    fn update_config_drops_a_dropped_lgpm_repo() {
        let original = minimal_v0_2_0()
            + "\n[repos.lgpm]\nsource = \"https://github.com/logos-co/lgpm.git\"\n\
               pin = \"bb237766baf61404e12da86b7303cb41065464c9\"\nbuild = \"nix-flake\"\n";
        let mut cfg = parse_config(&original).expect("parse");
        assert!(cfg.lgpm_repo.is_some(), "fixture must start with one");
        cfg.lgpm_repo = None;

        let rewritten = update_config(&original, &cfg).expect("update");
        assert!(
            !rewritten.contains("[repos.lgpm]"),
            "dropped lgpm table survived:\n{rewritten}"
        );
        assert!(
            parse_config(&rewritten)
                .expect("reparse")
                .lgpm_repo
                .is_none(),
            "dropped lgpm reparsed as present:\n{rewritten}"
        );
        assert!(rewritten.contains("[repos.lez]"), "{rewritten}");
    }

    /// `prune_run_profiles` is the defensive twin of the `[basecamp]` prune.
    /// Its own doc comment concedes it is not load-bearing today — which is
    /// exactly why it needs a test: nothing else stops a future reader from
    /// "simplifying" it into the blanket remove its basecamp analogue must
    /// never become.
    #[test]
    fn update_config_keeps_unmodelled_keys_in_a_cleared_run_profile() {
        let original = minimal_v0_2_0()
            + "\n[run]\nreset = true\n\
               \n[run.profiles.dev]\nreset = true\n\
               # our harness reads this; scaffold does not model it\n\
               harness_tag = \"nightly\"\n";
        let mut cfg = parse_config(&original).expect("parse");
        cfg.run = RunConfig::default();

        let rewritten = update_config(&original, &cfg).expect("update");
        assert!(
            rewritten.contains("harness_tag = \"nightly\""),
            "an unmodelled key in a cleared run profile was deleted:\n{rewritten}"
        );
        assert!(
            rewritten.contains("# our harness reads this; scaffold does not model it"),
            "the comment explaining it went with it:\n{rewritten}"
        );
        let back = parse_config(&rewritten).expect("reparse");
        assert!(
            !back.run.profiles.get("dev").is_some_and(|p| p.reset),
            "the owned key survived a reset to default:\n{rewritten}"
        );
    }

    /// A per-profile `env` key removed from the model must go, while its
    /// siblings stay. `parse_string_map` is total, so this retain is
    /// legitimately model-keyed — the risk here is a stale key, not deletion.
    #[test]
    fn update_config_prunes_one_key_from_a_basecamp_profile_env() {
        let original = minimal_v0_2_0()
            + "\n[basecamp.profiles.alice]\nruntime_dir = \"/run/user/1000\"\n\
               \n[basecamp.profiles.alice.env]\nA = \"1\"\nB = \"2\"\n";
        let mut cfg = parse_config(&original).expect("parse");
        cfg.basecamp
            .as_mut()
            .expect("basecamp")
            .profiles
            .get_mut("alice")
            .expect("alice")
            .env
            .remove("B");

        let rewritten = update_config(&original, &cfg).expect("update");
        let back = parse_config(&rewritten).expect("reparse");
        let env = &back
            .basecamp
            .as_ref()
            .expect("basecamp")
            .profiles
            .get("alice")
            .expect("alice")
            .env;
        assert_eq!(env.get("A").map(String::as_str), Some("1"), "{rewritten}");
        assert!(
            !env.contains_key("B"),
            "stale env key survived:\n{rewritten}"
        );
    }

    /// Writing the same config twice must converge: the second rewrite of an
    /// already-rendered file changes nothing.
    #[test]
    fn update_config_is_idempotent() {
        // Every section with conditional emit logic, plus comments and an
        // unmodelled section — the minimal fixture alone would skip all of the
        // branches that could oscillate between two renderings.
        let original = format!(
            "# leading comment\n{}\n\
             [basecamp]\nport_base = 41000\n\n[basecamp.env]\nFOO = \"bar\"\n\
             \n[basecamp.profiles.alice]\nruntime_dir = \"/run/user/1000\"\n\
             \n[modules.alpha]\nflake = \"./a#lgx\"\nrole = \"project\"\n\
             standalone_app = \"app\"\n\
             \n[run]\nreset = true\npost_deploy = \"echo hi\"\n\
             \n[run.watch]\ninclude = [\"src/**\"]\n\
             \n[unknown_section]\nkey = \"value\"\n",
            minimal_v0_2_0()
        );
        let cfg = parse_config(&original).expect("parse");
        let once = update_config(&original, &cfg).expect("first");
        let twice = update_config(&once, &parse_config(&once).expect("reparse")).expect("second");
        assert_eq!(
            once, twice,
            "a second save must be a no-op; churn here rewrites the user's \
             file on every command"
        );
        // And the first pass must not have lost anything on the way.
        assert!(once.contains("# leading comment"), "{once}");
        assert!(once.contains("[unknown_section]"), "{once}");
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
        assert!(err.to_string().contains("init"), "{err}");
        let doc: DocumentMut = toml.parse().expect("re-parse for markers");
        let markers = detect_old_schema_markers(&doc, "0.2.0");
        assert!(markers.has_lssa, "{markers:?}");
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
    fn parse_config_with_run_watch_section() {
        let toml = minimal_v0_2_0()
            + "[run.watch]\ninclude = [\"programs/**/guest/**\"]\nexclude = [\"**/*.md\", \"Cargo.lock\"]\ndebounce_ms = 1500\n";
        let cfg = parse_config(&toml).expect("parse");
        assert_eq!(
            cfg.run.watch.include,
            vec!["programs/**/guest/**".to_string()]
        );
        assert_eq!(
            cfg.run.watch.exclude,
            vec!["**/*.md".to_string(), "Cargo.lock".to_string()]
        );
        assert_eq!(cfg.run.watch.debounce_ms, Some(1500));
    }

    #[test]
    fn parse_config_run_watch_rejects_empty_glob() {
        // An empty pattern normalizes to match-all; an empty `exclude` would
        // silently suppress every watch trigger, so it's rejected at parse.
        let toml = minimal_v0_2_0() + "[run.watch]\nexclude = [\"\"]\n";
        let err = parse_config(&toml).unwrap_err();
        assert!(err.to_string().contains("must not be empty"), "{err}");
    }

    #[test]
    fn parse_config_run_watch_rejects_negative_debounce() {
        let toml = minimal_v0_2_0() + "[run.watch]\ndebounce_ms = -5\n";
        let err = parse_config(&toml).unwrap_err();
        assert!(err.to_string().contains("debounce_ms"), "{err}");
    }

    #[test]
    fn run_watch_round_trips_through_parse_serialize() {
        let toml = minimal_v0_2_0()
            + "[run.watch]\ninclude = [\"src/**\"]\nexclude = [\"**/target/**\"]\ndebounce_ms = 750\n";
        let cfg1 = parse_config(&toml).expect("parse");
        let serialized = serialize_config(&cfg1).expect("serialize");
        let cfg2 = parse_config(&serialized).expect("re-parse");
        assert_eq!(cfg2.run.watch.include, vec!["src/**".to_string()]);
        assert_eq!(cfg2.run.watch.exclude, vec!["**/target/**".to_string()]);
        assert_eq!(cfg2.run.watch.debounce_ms, Some(750));
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
    fn run_profile_deploy_defaults_to_true() {
        // Absent `deploy` key → deploy runs, preserving historical behavior.
        let toml = minimal_v0_2_0() + "[run.profiles.demo]\npost_deploy = \"echo demo\"\n";
        let cfg = parse_config(&toml).expect("parse");
        assert!(cfg.run.profiles.get("demo").expect("demo present").deploy);
        // The inline/default profile also defaults deploy to true.
        assert!(RunProfile::default().deploy);
        assert!(cfg.run.inline.deploy);
    }

    #[test]
    fn parse_config_run_profile_deploy_false() {
        let toml = minimal_v0_2_0()
            + "[run.profiles.demo]\ndeploy = false\npost_deploy = [\"scripts/self-deploy.sh\"]\n";
        let cfg = parse_config(&toml).expect("parse");
        let demo = cfg.run.profiles.get("demo").expect("demo present");
        assert!(!demo.deploy);
        assert_eq!(demo.post_deploy, vec!["scripts/self-deploy.sh".to_string()]);
    }

    #[test]
    fn parse_config_inline_run_deploy_false() {
        let toml = minimal_v0_2_0() + "[run]\ndeploy = false\n";
        let cfg = parse_config(&toml).expect("parse");
        assert!(!cfg.run.inline.deploy);
        let resolved = cfg.run.resolve_profile(None).expect("resolve");
        assert!(!resolved.deploy);
    }

    #[test]
    fn run_profile_deploy_round_trips_through_parse_serialize() {
        let toml = minimal_v0_2_0()
            + "[run]\ndeploy = false\n[run.profiles.demo]\ndeploy = false\npost_deploy = [\"echo demo\"]\n";
        let cfg1 = parse_config(&toml).expect("parse");
        let serialized = serialize_config(&cfg1).expect("serialize");
        let cfg2 = parse_config(&serialized).expect("re-parse");
        assert!(!cfg2.run.inline.deploy);
        let demo = cfg2.run.profiles.get("demo").expect("demo present");
        assert!(!demo.deploy);
        assert_eq!(demo.post_deploy, vec!["echo demo".to_string()]);
    }

    #[test]
    fn run_profile_deploy_true_is_not_serialized() {
        // The `true` default must not be emitted, to keep scaffold.toml minimal.
        let toml = minimal_v0_2_0() + "[run.profiles.demo]\npost_deploy = [\"echo demo\"]\n";
        let cfg = parse_config(&toml).expect("parse");
        let serialized = serialize_config(&cfg).expect("serialize");
        assert!(
            !serialized.contains("deploy = true"),
            "default deploy=true should not be serialized:\n{serialized}"
        );
    }

    #[test]
    fn run_profile_topup_defaults_to_true() {
        // Absent `topup` key → topup runs, preserving historical behavior.
        let toml = minimal_v0_2_0() + "[run.profiles.demo]\npost_deploy = \"echo demo\"\n";
        let cfg = parse_config(&toml).expect("parse");
        assert!(cfg.run.profiles.get("demo").expect("demo present").topup);
        // The inline/default profile also defaults topup to true.
        assert!(RunProfile::default().topup);
        assert!(cfg.run.inline.topup);
    }

    #[test]
    fn parse_config_run_profile_topup_false() {
        let toml = minimal_v0_2_0()
            + "[run.profiles.demo]\ntopup = false\npost_deploy = [\"cargo run --bin demo\"]\n";
        let cfg = parse_config(&toml).expect("parse");
        let demo = cfg.run.profiles.get("demo").expect("demo present");
        assert!(!demo.topup);
        assert_eq!(demo.post_deploy, vec!["cargo run --bin demo".to_string()]);
    }

    #[test]
    fn parse_config_inline_run_topup_false() {
        let toml = minimal_v0_2_0() + "[run]\ntopup = false\n";
        let cfg = parse_config(&toml).expect("parse");
        assert!(!cfg.run.inline.topup);
        let resolved = cfg.run.resolve_profile(None).expect("resolve");
        assert!(!resolved.topup);
    }

    #[test]
    fn run_profile_topup_round_trips_through_parse_serialize() {
        let toml = minimal_v0_2_0()
            + "[run]\ntopup = false\n[run.profiles.demo]\ntopup = false\npost_deploy = [\"echo demo\"]\n";
        let cfg1 = parse_config(&toml).expect("parse");
        let serialized = serialize_config(&cfg1).expect("serialize");
        let cfg2 = parse_config(&serialized).expect("re-parse");
        assert!(!cfg2.run.inline.topup);
        let demo = cfg2.run.profiles.get("demo").expect("demo present");
        assert!(!demo.topup);
        assert_eq!(demo.post_deploy, vec!["echo demo".to_string()]);
    }

    #[test]
    fn run_profile_topup_true_is_not_serialized() {
        // The `true` default must not be emitted, to keep scaffold.toml minimal.
        let toml = minimal_v0_2_0() + "[run.profiles.demo]\npost_deploy = [\"echo demo\"]\n";
        let cfg = parse_config(&toml).expect("parse");
        let serialized = serialize_config(&cfg).expect("serialize");
        assert!(
            !serialized.contains("topup = true"),
            "default topup=true should not be serialized:\n{serialized}"
        );
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
                deploy: true,
                topup: true,
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
