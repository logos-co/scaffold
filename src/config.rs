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
    LocalnetConfig,
    ModuleEntry, ModuleRole, RepoBuild, RepoRef, RunConfig, RunProfile, WatchConfig,
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

    let watch = parse_run_watch(run_table)?;

    Ok(RunConfig {
        default_profile,
        inline: RunProfile {
            reset: inline_reset,
            post_deploy: inline_post_deploy,
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
    }
    check_toml_value("circuits.install_dir", &install_dir)?;

    Ok(CircuitsConfig {
        version,
        url_template,
        install_dir,
    })
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
        if let Some(app) = &entry.standalone_app {
            check_toml_value(&format!("modules.{name}.standalone_app"), app)?;
            table["standalone_app"] = value(app);
        }
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

    // [circuits]
    check_toml_value("circuits.version", &cfg.circuits.version)?;
    if let Some(template) = &cfg.circuits.url_template {
        check_toml_value("circuits.url_template", template)?;
    }
    check_toml_value("circuits.install_dir", &cfg.circuits.install_dir)?;
    let circuits = doc.entry("circuits").or_insert(Item::Table(Table::new()));
    let circuits_table = circuits.as_table_mut().expect("circuits table");
    circuits_table["version"] = value(&cfg.circuits.version);
    if let Some(template) = &cfg.circuits.url_template {
        circuits_table["url_template"] = value(template);
    }
    if cfg.circuits.install_dir != CircuitsConfig::default().install_dir {
        circuits_table["install_dir"] = value(&cfg.circuits.install_dir);
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
        let basecamp_table = basecamp.as_table_mut().expect("basecamp table");
        // Only emit port keys when they differ from the defaults, so setting
        // just `[basecamp.env]` doesn't churn a user's scaffold.toml with
        // default `port_base`/`port_stride` on the next `save_project_config`.
        let default_bc = BasecampConfig::default();
        let mut wrote_direct_key = false;
        if bc.port_base != default_bc.port_base {
            basecamp_table["port_base"] = value(i64::from(bc.port_base));
            wrote_direct_key = true;
        }
        if bc.port_stride != default_bc.port_stride {
            basecamp_table["port_stride"] = value(i64::from(bc.port_stride));
            wrote_direct_key = true;
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
            for (k, v) in &bc.env {
                env_table[k] = value(v);
            }
        }
        if !bc.env_append.is_empty() {
            let append_table = child_table(basecamp_table, "env_append");
            for (k, list) in &bc.env_append {
                append_table[k] = string_array(list);
            }
        }
        if !bc.profiles.is_empty() {
            let profiles = child_table(basecamp_table, "profiles");
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
                if let Some(f) = &p.env_file {
                    profile_table["env_file"] = value(f);
                    wrote_scalar = true;
                }
                if let Some(d) = &p.runtime_dir {
                    profile_table["runtime_dir"] = value(d);
                    wrote_scalar = true;
                }
                if let Some(l) = &p.log_file {
                    profile_table["log_file"] = value(l);
                    wrote_scalar = true;
                }
                if !p.env.is_empty() {
                    let env_table = child_table(profile_table, "env");
                    for (k, v) in &p.env {
                        env_table[k] = value(v);
                    }
                }
                if !wrote_scalar {
                    profile_table.set_implicit(true);
                }
            }
        }
    }

    // [run] — only emit when non-default to keep fresh scaffold.toml minimal.
    write_run_config(&mut doc, &cfg.run)?;

    Ok(doc.to_string())
}

fn write_run_config(doc: &mut DocumentMut, run: &RunConfig) -> DynResult<()> {
    let has_inline = run.inline.reset || !run.inline.post_deploy.is_empty();
    let has_default_profile = run.default_profile.is_some();
    let has_profiles = !run.profiles.is_empty();
    let has_watch = run.watch != WatchConfig::default();
    if !has_inline && !has_default_profile && !has_profiles && !has_watch {
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

    if has_watch {
        for g in run.watch.include.iter().chain(run.watch.exclude.iter()) {
            check_toml_value("run.watch", g)?;
        }
        let watch_table = ensure_subtable(doc, "run", "watch");
        if !run.watch.include.is_empty() {
            watch_table["include"] = string_array(&run.watch.include);
        }
        if !run.watch.exclude.is_empty() {
            watch_table["exclude"] = string_array(&run.watch.exclude);
        }
        if let Some(ms) = run.watch.debounce_ms {
            watch_table["debounce_ms"] = value(ms as i64);
        }
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
    if repo.build != RepoBuild::default() {
        table["build"] = value(repo.build.as_str());
    } else {
        table.remove("build");
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
        table.remove("attr");
    }
    if !repo.path.is_empty() {
        table["path"] = value(&repo.path);
    } else {
        table.remove("path");
    }
    Ok(())
}

/// Get or create a child `Table` under an existing `Table` without touching the
/// parent's implicit flag — unlike `ensure_subtable`, which marks its parent
/// implicit (wrong when the parent has real keys, e.g. `[basecamp]`).
fn child_table<'a>(parent: &'a mut Table, name: &str) -> &'a mut Table {
    parent
        .entry(name)
        .or_insert(Item::Table(Table::new()))
        .as_table_mut()
        .expect("child is a table")
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
