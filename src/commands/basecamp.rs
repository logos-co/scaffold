use std::collections::{BTreeMap, HashSet, VecDeque};
use std::ffi::OsString;
use std::fs;
use std::os::unix::process::CommandExt;
use std::path::{Component, Path, PathBuf, MAIN_SEPARATOR};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{bail, Context};

use crate::config::{default_basecamp_repo, default_lgpm_repo};
use crate::constants::{
    BASECAMP_ATTR, BASECAMP_AUTODISCOVER_SKIP_SUBDIRS, BASECAMP_DEPENDENCIES,
    BASECAMP_PREINSTALLED_MODULES, BASECAMP_PROFILES_REL, BASECAMP_PROFILE_ALICE,
    BASECAMP_PROFILE_BOB, BASECAMP_SOURCE, BASECAMP_XDG_APP_SUBPATH, DEFAULT_BASECAMP_PIN,
    DEFAULT_LGPM_PIN, LGPM_ATTR, LGPM_SOURCE,
};
use crate::model::{
    BasecampSource, BasecampState, ModuleEntry, ModuleRole, Project, RepoBuild, RepoRef,
};
use crate::process::{derive_log_path, run_checked, run_logged, set_print_output};
use crate::project::{load_project, resolve_cache_root, save_project_config};
use crate::repo::{sync_repo_to_pin_at_path_with_opts, RepoSyncOptions};
use crate::state::{read_basecamp_state, write_basecamp_state};
use crate::DynResult;

#[derive(Debug, Clone)]
#[allow(dead_code)] // Fields wired up in later phases
pub(crate) enum BasecampAction {
    Setup,
    Modules {
        paths: Vec<PathBuf>,
        flakes: Vec<String>,
        show: bool,
    },
    /// Pure replay: build and install everything captured in `state` (deps
    /// first). No source-set flags — use `basecamp modules` to change what's
    /// captured. If state is empty, transparently invokes `modules` in
    /// auto-discover mode and proceeds.
    Install {
        print_output: bool,
    },
    Launch {
        profile: String,
        no_clean: bool,
        yes: bool,
        dry_run: bool,
    },
    /// Attr-swap replay on `state.project_sources` only (`#lgx` →
    /// `#lgx-portable`). `state.dependencies` is ignored — the target AppImage
    /// provides those. No CLI source flags.
    BuildPortable,
    /// Basecamp-specific doctor: captured modules summary, manifest variant
    /// check per seeded profile, and uncaptured-module drift against
    /// auto-discovery.
    Doctor {
        json: bool,
    },
    /// Print the canonical compatibility doc (`docs/basecamp-module-requirements.md`,
    /// embedded at compile time). Runnable outside a scaffold project so LLMs
    /// exploring the CLI can retrieve the rules before setup.
    Docs,
}

pub(crate) fn cmd_basecamp(action: BasecampAction) -> DynResult<()> {
    // `docs` is project-context-free: it prints a bundled doc so an LLM (or
    // human) can retrieve compatibility rules even before running `init`.
    if matches!(action, BasecampAction::Docs) {
        return cmd_basecamp_docs();
    }

    let project = load_project().context(
        "This command must be run inside a logos-scaffold project.\nNext step: cd into your scaffolded project directory and retry.",
    )?;

    match action {
        BasecampAction::Setup => cmd_basecamp_setup(project),
        BasecampAction::Modules {
            paths,
            flakes,
            show,
        } => cmd_basecamp_modules(project, paths, flakes, show, &NixLgxProbe),
        BasecampAction::Install { print_output } => {
            if print_output {
                set_print_output(true);
            }
            cmd_basecamp_install(project, &NixLgxProbe)
        }
        BasecampAction::Launch {
            profile,
            no_clean,
            yes,
            dry_run,
        } => cmd_basecamp_launch(project, profile, no_clean, yes, dry_run),
        BasecampAction::BuildPortable => cmd_basecamp_build_portable(project),
        BasecampAction::Doctor { json } => cmd_basecamp_doctor(project, json),
        // Handled above via early return (project-context-free).
        BasecampAction::Docs => unreachable!("handled before load_project"),
    }
}

/// Canonical compatibility doc bundled at compile time. Single source of
/// truth: the markdown file at `docs/basecamp-module-requirements.md`.
const BASECAMP_MODULE_REQUIREMENTS_DOC: &str =
    include_str!("../../docs/basecamp-module-requirements.md");

/// Text used everywhere we want to point an LLM at the full compatibility
/// rules — error hints, help breadcrumbs, doctor output. Keep it short and
/// consistent so agents can pattern-match.
pub(crate) const COMPAT_DOCS_BREADCRUMB: &str =
    "See `logos-scaffold basecamp docs` for project compatibility rules.";

fn cmd_basecamp_docs() -> DynResult<()> {
    print!("{BASECAMP_MODULE_REQUIREMENTS_DOC}");
    Ok(())
}

fn cmd_basecamp_setup(mut project: Project) -> DynResult<()> {
    // Pull or default-fill [repos.basecamp]. If the project has never been
    // through `lgs new` post-0.2.0 and lacks the section, fill it with the
    // canonical default and persist back.
    let mut basecamp_repo = project
        .config
        .basecamp_repo
        .clone()
        .unwrap_or_else(|| default_basecamp_repo(DEFAULT_BASECAMP_PIN));
    if basecamp_repo.source.is_empty() {
        basecamp_repo.source = BASECAMP_SOURCE.to_string();
    }
    if basecamp_repo.pin.is_empty() {
        basecamp_repo.pin = DEFAULT_BASECAMP_PIN.to_string();
    }
    if basecamp_repo.attr.is_empty() {
        basecamp_repo.attr = BASECAMP_ATTR.to_string();
    }

    // Same defaults for lgpm.
    let mut lgpm_repo = project
        .config
        .lgpm_repo
        .clone()
        .unwrap_or_else(|| default_lgpm_repo(DEFAULT_LGPM_PIN));
    if lgpm_repo.source.is_empty() {
        lgpm_repo.source = LGPM_SOURCE.to_string();
    }
    if lgpm_repo.pin.is_empty() {
        lgpm_repo.pin = DEFAULT_LGPM_PIN.to_string();
    }
    if lgpm_repo.attr.is_empty() {
        lgpm_repo.attr = LGPM_ATTR.to_string();
    }

    let (cache_root, _) = resolve_cache_root(&project)?;
    let basecamp_repo_path = cache_root.join("repos/basecamp").join(&basecamp_repo.pin);

    println!("cloning basecamp at {}", &basecamp_repo.pin);
    sync_repo_to_pin_at_path_with_opts(
        &basecamp_repo_path,
        &basecamp_repo.source,
        &basecamp_repo.pin,
        "basecamp",
        RepoSyncOptions::auto_reclone_cache_repo(),
    )?;

    let pin_artifacts = cache_root.join("basecamp").join(&basecamp_repo.pin);
    fs::create_dir_all(&pin_artifacts)
        .with_context(|| format!("create {}", pin_artifacts.display()))?;

    let basecamp_bin = build_basecamp_app(&project.root, &basecamp_repo_path, &pin_artifacts)?;
    // lgpm is built from a flake ref derived from [repos.lgpm].
    let lgpm_flake_ref = format_flake_ref(&lgpm_repo);
    let lgpm_bin = build_lgpm(&project.root, &pin_artifacts, &lgpm_flake_ref)?;

    let profiles_root = project.root.join(BASECAMP_PROFILES_REL);
    let seeded = seed_profiles(
        &profiles_root,
        &[BASECAMP_PROFILE_ALICE, BASECAMP_PROFILE_BOB],
    )?;
    println!("seeded profiles: {}", seeded.join(", "));

    let state_path = project.root.join(".scaffold/state/basecamp.state");
    let state = BasecampState {
        pin: basecamp_repo.pin.clone(),
        basecamp_bin: basecamp_bin.display().to_string(),
        lgpm_bin: lgpm_bin.display().to_string(),
    };
    write_basecamp_state(&state_path, &state)?;

    project.config.basecamp_repo = Some(basecamp_repo);
    project.config.lgpm_repo = Some(lgpm_repo);
    save_project_config(&project)?;

    println!("setup complete");
    Ok(())
}

/// Build a flake ref string from a `[repos.<name>]` entry where `build ==
/// NixFlake`. Format: `<source>/<pin>#<attr>` (matches the flake-ref format
/// the legacy `[basecamp].lgpm_flake` string used).
fn format_flake_ref(repo: &RepoRef) -> String {
    debug_assert!(repo.build == RepoBuild::NixFlake);
    if repo.attr.is_empty() {
        format!("{}/{}", repo.source, repo.pin)
    } else {
        format!("{}/{}#{}", repo.source, repo.pin, repo.attr)
    }
}

fn build_basecamp_app(project_root: &Path, repo: &Path, out_dir: &Path) -> DynResult<PathBuf> {
    let link = out_dir.join("app-result");
    let log = derive_log_path(project_root, "setup-basecamp");
    let mut cmd = Command::new("nix");
    cmd.current_dir(repo)
        .arg("build")
        .arg(".#app")
        .arg("--out-link")
        .arg(&link);
    run_logged(&mut cmd, "building basecamp", &log)?;
    resolve_basecamp_binary(&link)
}

fn build_lgpm(project_root: &Path, out_dir: &Path, flake_ref: &str) -> DynResult<PathBuf> {
    let link = out_dir.join("lgpm-result");
    let log = derive_log_path(project_root, "setup-lgpm");
    let mut cmd = Command::new("nix");
    cmd.arg("build").arg(flake_ref).arg("--out-link").arg(&link);
    run_logged(&mut cmd, &format!("building lgpm ({flake_ref})"), &log)?;
    Ok(link.join("bin/lgpm"))
}

fn resolve_basecamp_binary(app_link: &Path) -> DynResult<PathBuf> {
    // v0.1.1 layout: bin/logos-basecamp (Linux); macOS app bundle ships under Applications/.
    for rel in ["bin/logos-basecamp", "bin/LogosBasecamp", "bin/basecamp"] {
        let candidate = app_link.join(rel);
        if candidate.exists() {
            return Ok(candidate);
        }
    }
    let platform_hint = if cfg!(target_os = "macos") {
        "\nNote: on macOS, `nix build .#app` produces an app bundle under Applications/. \
         v0.1.x does not yet expose a CLI-invocable binary on macOS; track basecamp-profiles spec §6."
    } else {
        ""
    };
    bail!(
        "could not locate basecamp binary inside nix build result {}{platform_hint}",
        app_link.display()
    )
}

// Concurrent `launch <same-profile>` is undefined per spec §2.3 ("v1 does not
// lock; document as 'don't do that'"). The code below assumes a single launcher
// per profile at a time — scrub, re-seed, replay install, and PID write are all
// non-atomic. If two invocations race, expect partial state.
fn cmd_basecamp_launch(
    project: Project,
    profile: String,
    no_clean: bool,
    yes: bool,
    dry_run: bool,
) -> DynResult<()> {
    let state_path = project.root.join(".scaffold/state/basecamp.state");
    let state = match read_basecamp_state(&state_path).ok() {
        Some(s) if !s.basecamp_bin.is_empty() && !s.lgpm_bin.is_empty() => s,
        _ => bail!("basecamp not set up yet; run: logos-scaffold basecamp setup"),
    };
    if !Path::new(&state.basecamp_bin).exists() || !Path::new(&state.lgpm_bin).exists() {
        bail!("basecamp not set up yet; run: logos-scaffold basecamp setup");
    }

    if profile != BASECAMP_PROFILE_ALICE && profile != BASECAMP_PROFILE_BOB {
        bail!(
            "unknown profile `{profile}`; v1 only supports `{}` and `{}`",
            BASECAMP_PROFILE_ALICE,
            BASECAMP_PROFILE_BOB
        );
    }
    // Defense-in-depth against a future caller bypassing the allowlist: refuse
    // any profile name that could escape the profiles root via path separators.
    if profile.contains('/') || profile.contains(MAIN_SEPARATOR) {
        bail!("profile name `{profile}` must not contain path separators");
    }

    let profiles_root = project.root.join(BASECAMP_PROFILES_REL);
    let profile_dir = profiles_root.join(&profile);
    if !profile_dir.is_dir() {
        bail!(
            "profile `{profile}` missing under {}; re-run `logos-scaffold basecamp setup`",
            profiles_root.display()
        );
    }

    if dry_run {
        return cmd_basecamp_launch_dry_run(&project, &profile, &profile_dir, &state, no_clean);
    }

    // The default launch scrubs `xdg-data` and `xdg-cache` for the profile and
    // reinstalls captured modules from scratch. That destroys per-profile state
    // (chat history, settings, lgpm install records) — so refuse the default
    // form unless the caller has explicitly confirmed via `--yes`. `--no-clean`
    // is a non-destructive launch and does not require confirmation.
    if !no_clean && !yes {
        bail!(
            "basecamp launch is destructive by default: it scrubs the profile's xdg-data and xdg-cache, then reinstalls captured modules.\n\
             Pass --yes to confirm, --dry-run to preview the plan, or --no-clean to launch without scrubbing.\n\
             Examples:\n  \
             logos-scaffold basecamp launch {profile} --dry-run\n  \
             logos-scaffold basecamp launch {profile} --yes\n  \
             logos-scaffold basecamp launch {profile} --no-clean"
        );
    }

    let launch_state_path = profile_dir.join("launch.state");
    let expected_comm = basecamp_comm_name(&state.basecamp_bin);
    if let Some(pid) = read_launch_pid(&launch_state_path) {
        // PID reuse means we can't be sure the recorded PID still maps to basecamp.
        // Only issue signals if the PID's current comm matches; `kill_process_tree`
        // re-verifies the comm before each signal to reduce the TOCTOU window.
        if pid_comm_matches(pid, &expected_comm) {
            kill_process_tree(pid, &expected_comm);
        }
        let _ = fs::remove_file(&launch_state_path);
    }

    // Clean-slate launch replays `[basecamp.modules]` into the freshly-scrubbed
    // profile. An empty capture set would make that replay a silent no-op, so the
    // profile would come up with zero modules — violating the clean-slate
    // guarantee. Fail fast with a hint. `--no-clean` skips this check because
    // that mode deliberately preserves whatever's already installed.
    if !no_clean && total_captured_modules(&project) == 0 {
        bail!(
            "no modules captured — run `logos-scaffold basecamp modules` before launching, \
             or pass `--no-clean` to keep the currently-installed module set."
        );
    }

    // seed_profiles is idempotent (tested) and cheap — always run it so a prior
    // crash mid-scrub doesn't leave the profile without its xdg subdirs.
    seed_profiles(&profiles_root, &[profile.as_str()])?;
    if !no_clean {
        scrub_profile_data_and_cache(&project.root, &profile_dir)?;
        // Re-seed after scrub: scrub removed xdg-data + xdg-cache; put their
        // module/plugin subtrees back before lgpm writes into them.
        seed_profiles(&profiles_root, &[profile.as_str()])?;
        let (cache_root, _) = resolve_cache_root(&project)?;
        install_sources_into_profiles(
            &project,
            &state,
            &cache_root,
            &profiles_root,
            &[profile.clone()],
        )?;
    }

    // Variant pre-flight: warn if any installed module is missing the current
    // platform's `<plat>-dev` manifest.json `main` key. Basecamp v0.1.1's
    // variant resolver silently hangs on first click when loading such a
    // plugin, so catch it before the user ever clicks. Non-blocking: we
    // still exec basecamp regardless — the dev may want to click around
    // anyway.
    if let Some(expected_dev) = platform_dev_variant_key() {
        let issues = check_manifest_variants(&profile_dir, &profile, expected_dev);
        let module_count = count_installed_modules(&profile_dir);
        if issues.is_empty() {
            println!(
                "launch: profile {profile} has {module_count} module(s); all {expected_dev} variants present ✓"
            );
        } else {
            let names: Vec<String> = issues.iter().map(|i| i.module_name.clone()).collect();
            println!(
                "launch: profile {profile} has {module_count} module(s); \
                 {} missing {expected_dev} variant ({}); \
                 run `logos-scaffold doctor` for details — plugins will hang on click.",
                issues.len(),
                names.join(", ")
            );
        }
    }

    let env = launch_env(&profile_dir, &profile);
    println!("launching basecamp for profile {profile}");
    let mut cmd = Command::new(&state.basecamp_bin);
    for (k, v) in &env {
        cmd.env(k, v);
    }
    // Per-module port-override env vars (spec §3.4) are owned by each module and
    // flow in via a registry — empty in v1 since no modules have published names.
    // Concurrent alice/bob on the same host may collide on module-level ports
    // until upstreams adopt overrides; see basecamp-profiles §3.4.
    write_launch_pid(&launch_state_path, std::process::id())?;
    let err = cmd.exec();
    // exec() only returns on failure. On Linux/Unix exec preserves the PID, so
    // launch.state is valid once exec succeeds — but on failure the PID we wrote
    // belongs to the scaffold process that's about to exit. Remove the file so a
    // later launch doesn't kill whatever reuses the PID.
    let _ = fs::remove_file(&launch_state_path);
    bail!("failed to exec basecamp at {}: {err}", state.basecamp_bin);
}

fn cmd_basecamp_launch_dry_run(
    project: &Project,
    profile: &str,
    profile_dir: &Path,
    state: &BasecampState,
    no_clean: bool,
) -> DynResult<()> {
    let xdg_data = profile_dir.join("xdg-data");
    let xdg_cache = profile_dir.join("xdg-cache");

    println!("dry-run: basecamp launch {profile} (no changes made)");
    if no_clean {
        println!("planned: skip scrub (--no-clean); preserve existing profile data");
    } else {
        println!(
            "planned: scrub {} (exists: {})",
            xdg_data.display(),
            xdg_data.exists()
        );
        println!(
            "planned: scrub {} (exists: {})",
            xdg_cache.display(),
            xdg_cache.exists()
        );
        let captured = total_captured_modules(project);
        if captured == 0 {
            println!(
                "warning: no modules captured — a real launch would abort with `no modules captured` before reinstall. \
                 Run `logos-scaffold basecamp modules` first."
            );
        } else {
            println!(
                "planned: reinstall {captured} captured module(s) into profile {profile} via lgpm"
            );
        }
    }
    println!(
        "planned: exec basecamp at {} for profile {profile}",
        state.basecamp_bin
    );
    Ok(())
}

/// Env map exported to the basecamp child on launch. Scaffold-owned names only
/// (spec §3.4); module port-override vars are not yet registered.
fn launch_env(profile_dir: &Path, profile_name: &str) -> BTreeMap<String, OsString> {
    let mut env = BTreeMap::new();
    env.insert(
        "XDG_CONFIG_HOME".into(),
        profile_dir.join("xdg-config").into_os_string(),
    );
    env.insert(
        "XDG_DATA_HOME".into(),
        profile_dir.join("xdg-data").into_os_string(),
    );
    env.insert(
        "XDG_CACHE_HOME".into(),
        profile_dir.join("xdg-cache").into_os_string(),
    );
    env.insert("LOGOS_PROFILE".into(), profile_name.into());
    env
}

/// Remove a profile's `xdg-data` and `xdg-cache` trees. Refuses to operate on any
/// path outside `<project>/.scaffold/basecamp/profiles/` — guards against a
/// caller that constructs an absolute profile_dir pointing elsewhere.
fn scrub_profile_data_and_cache(project_root: &Path, profile_dir: &Path) -> DynResult<()> {
    let safe_root = project_root.join(BASECAMP_PROFILES_REL);
    // canonicalize() requires every path component to exist. Create safe_root up
    // front so the canonical form is well-defined before the prefix check. For
    // profile_dir we canonicalize the parent (which must exist — we just made it)
    // and append the final component, matching how the path would resolve.
    fs::create_dir_all(&safe_root).with_context(|| format!("create {}", safe_root.display()))?;
    let canon_safe = safe_root
        .canonicalize()
        .with_context(|| format!("canonicalize {}", safe_root.display()))?;
    let canon_profile = canonicalize_under(profile_dir)?;
    if !canon_profile.starts_with(&canon_safe) {
        bail!(
            "refusing to scrub {} — outside the profiles root {}",
            canon_profile.display(),
            canon_safe.display()
        );
    }
    for xdg in ["xdg-data", "xdg-cache"] {
        let dir = profile_dir.join(xdg);
        if dir.exists() {
            fs::remove_dir_all(&dir).with_context(|| format!("scrub {}", dir.display()))?;
        }
    }
    Ok(())
}

/// Canonicalize `p`, allowing the final component not to exist yet (canonicalize
/// the parent and re-append). Returns an error if the parent is missing or a
/// symlink to somewhere that doesn't resolve — never silently falls back.
fn canonicalize_under(p: &Path) -> DynResult<PathBuf> {
    if let Ok(c) = p.canonicalize() {
        return Ok(c);
    }
    let parent = p
        .parent()
        .ok_or_else(|| anyhow::anyhow!("path has no parent: {}", p.display()))?;
    let file = p
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("path has no final component: {}", p.display()))?;
    let canon_parent = parent
        .canonicalize()
        .with_context(|| format!("canonicalize {}", parent.display()))?;
    Ok(canon_parent.join(file))
}

fn read_launch_pid(path: &Path) -> Option<u32> {
    let text = fs::read_to_string(path).ok()?;
    for line in text.lines() {
        if let Some(rest) = line.trim().strip_prefix("pid=") {
            if let Ok(pid) = rest.parse::<u32>() {
                return Some(pid);
            }
        }
    }
    None
}

/// Atomic PID write via tmp-file + rename. A crash mid-write would otherwise
/// leave a truncated `launch.state` that reads back as malformed garbage.
fn write_launch_pid(path: &Path, pid: u32) -> DynResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let tmp = path.with_extension("state.tmp");
    fs::write(&tmp, format!("pid={pid}\n")).with_context(|| format!("write {}", tmp.display()))?;
    fs::rename(&tmp, path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))
}

/// `basename(basecamp_bin)` truncated to 15 bytes — matches `/proc/<pid>/comm`
/// semantics on Linux so [`pid_comm_matches`] can compare directly.
fn basecamp_comm_name(basecamp_bin: &str) -> String {
    let base = Path::new(basecamp_bin)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("basecamp");
    base.chars().take(15).collect()
}

/// True if the running process at `pid` has a comm matching `expected`. Uses
/// portable `ps -p` so this works on Linux and macOS. Returns false if `ps`
/// fails (process gone, PID reused by a process we can't inspect, etc.) — fail
/// closed: the caller skips the kill on a mismatch.
///
/// Both sides are truncated to 15 bytes (the kernel `/proc/<pid>/comm` limit)
/// before comparison, so a 20-byte binary name like `logos-basecamp-dev` still
/// matches a `comm` of `logos-basecamp-`. Equality is strict after truncation —
/// a prefix like `logos-bas` does not match `logos-basecamp-`.
fn pid_comm_matches(pid: u32, expected: &str) -> bool {
    let out = Command::new("ps")
        .arg("-p")
        .arg(pid.to_string())
        .arg("-o")
        .arg("comm=")
        .output();
    let Ok(out) = out else { return false };
    if !out.status.success() {
        return false;
    }
    let comm = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if comm.is_empty() {
        return false;
    }
    // ps may print a leading path (BSD) or just the comm (Linux). Compare
    // basenames and tolerate the 15-byte comm truncation either side.
    let comm_base = Path::new(&comm)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(&comm);
    let comm_trunc: String = comm_base.chars().take(15).collect();
    let expected_trunc: String = expected.chars().take(15).collect();
    !comm_trunc.is_empty() && comm_trunc == expected_trunc
}

/// Kill the process tree rooted at `pid`. Enumerates descendants via `/proc`
/// (Linux) and falls back to `pkill -P` on other Unix hosts where `/proc` isn't
/// available. Every signal is gated on a fresh `pid_comm_matches(pid, expected)`
/// check so a PID that was recycled between our entry check and the actual kill
/// doesn't get signalled at all. Descendants are always KILLed after the grace
/// period — the parent may exit before its children, leaving them orphaned to
/// init while still bound to profile ports.
fn kill_process_tree(pid: u32, expected_comm: &str) {
    // Snapshot descendants *before* TERM — once the parent starts exiting, its
    // children get reparented to init and we lose the ppid linkage.
    let descendants = collect_descendant_pids(pid);
    for child in &descendants {
        let _ = Command::new("kill")
            .arg("-TERM")
            .arg(child.to_string())
            .status();
    }
    if pid_comm_matches(pid, expected_comm) {
        let _ = Command::new("kill")
            .arg("-TERM")
            .arg(pid.to_string())
            .status();
    }
    let parent_exited = wait_for_exit(pid, Duration::from_millis(1500));
    for child in &descendants {
        let _ = Command::new("kill")
            .arg("-KILL")
            .arg(child.to_string())
            .status();
    }
    if !parent_exited && pid_comm_matches(pid, expected_comm) {
        let _ = Command::new("kill")
            .arg("-KILL")
            .arg(pid.to_string())
            .status();
        let _ = wait_for_exit(pid, Duration::from_millis(500));
    }
}

/// BFS over `/proc/<pid>/stat` to collect every descendant PID rooted at
/// `root`. On non-Linux hosts (no `/proc`), falls back to a one-level `pgrep -P`
/// — best-effort; grandchildren may be missed on macOS until v2 tracks them via
/// process groups.
fn collect_descendant_pids(root: u32) -> Vec<u32> {
    if Path::new("/proc").is_dir() {
        return linux_descendant_pids(root);
    }
    // Non-Linux fallback: direct children only.
    let out = Command::new("pgrep")
        .arg("-P")
        .arg(root.to_string())
        .output();
    let Ok(out) = out else { return Vec::new() };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.trim().parse::<u32>().ok())
        .collect()
}

fn linux_descendant_pids(root: u32) -> Vec<u32> {
    // Build child-map: ppid -> [pid...].
    let mut children: std::collections::HashMap<u32, Vec<u32>> = std::collections::HashMap::new();
    let Ok(entries) = fs::read_dir("/proc") else {
        return Vec::new();
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Ok(pid) = name.parse::<u32>() else {
            continue;
        };
        let Ok(stat) = fs::read_to_string(entry.path().join("stat")) else {
            continue;
        };
        // /proc/<pid>/stat layout: `<pid> (<comm>) <state> <ppid> ...`
        // `comm` can contain spaces/parens; split on the *last* ')'.
        let Some(rparen) = stat.rfind(')') else {
            continue;
        };
        let rest = stat[rparen + 1..].trim_start();
        let mut fields = rest.split_ascii_whitespace();
        let _state = fields.next();
        let Some(ppid_str) = fields.next() else {
            continue;
        };
        let Ok(ppid) = ppid_str.parse::<u32>() else {
            continue;
        };
        children.entry(ppid).or_default().push(pid);
    }

    let mut out = Vec::new();
    let mut seen: HashSet<u32> = HashSet::new();
    let mut queue: VecDeque<u32> = VecDeque::new();
    queue.push_back(root);
    while let Some(p) = queue.pop_front() {
        if let Some(kids) = children.get(&p) {
            for &k in kids {
                if seen.insert(k) {
                    out.push(k);
                    queue.push_back(k);
                }
            }
        }
    }
    out
}

fn wait_for_exit(pid: u32, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        // `kill -0` returns non-zero when the PID no longer exists (or we lack perms).
        let status = Command::new("kill").arg("-0").arg(pid.to_string()).status();
        if matches!(status, Ok(s) if !s.success()) || status.is_err() {
            return true;
        }
        thread::sleep(Duration::from_millis(100));
    }
    false
}

/// Build portable `.lgx` artefacts for hand-loading into a basecamp AppImage.
///
/// Operates on `state.project_sources` only, with each flake entry's `#lgx`
/// attribute swapped to `#lgx-portable`. `state.dependencies` is intentionally
/// ignored — the target AppImage provides its own (release/portable) copies
/// of companion modules via its in-app Package Manager catalog.
///
/// No CLI source flags: the source set lives in `state`, managed by
/// `basecamp modules`. If you want to produce a portable variant of something
/// that isn't a project source, `basecamp modules --flake <ref>#lgx` it first,
/// run `build-portable`, then revert with another `modules` call.
fn cmd_basecamp_build_portable(project: Project) -> DynResult<()> {
    let project_modules: std::collections::BTreeMap<String, ModuleEntry> = project
        .config
        .modules
        .iter()
        .filter(|(_, e)| e.role == ModuleRole::Project)
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    if project_modules.is_empty() {
        bail!(
            "no project modules captured in scaffold.toml; run `basecamp modules` \
             first (auto-discover) or `basecamp modules --flake <ref>#lgx \
             --path <file.lgx>` to capture explicitly. `build-portable` \
             operates on captured project sources only — it never discovers."
        );
    }

    // Order project modules so each appears AFTER its own project-internal
    // deps (leaves first). Basecamp loads `.lgx` artefacts in the order the
    // user hands them to it — emitting bottom-up means a module's deps are
    // already resolved by the time basecamp tries to resolve its symbols.
    let ordered_names = topo_order_project_modules(&project.root, &project_modules);

    // Rewrite each project source: attr-swap `#lgx` → `#lgx-portable` on
    // flake refs; pass Path sources through unchanged (they're pre-built).
    let portable_sources: Vec<BasecampSource> = ordered_names
        .iter()
        .map(|name| {
            let entry = &project_modules[name];
            let src = module_entry_to_source(&project.root, entry);
            match src {
                BasecampSource::Path(p) => BasecampSource::Path(p),
                BasecampSource::Flake(f) => {
                    BasecampSource::Flake(swap_flake_attr(&f, "lgx", "lgx-portable"))
                }
            }
        })
        .collect();

    // Local symlink dir: basecamp's AppImage "install lgx" button opens a
    // file picker starting in the project, and /nix/store/…-source paths
    // are painful to navigate by hand. Wipe + recreate so a re-run doesn't
    // leave stale symlinks from modules that have since been removed.
    let portable_dir = project.root.join(".scaffold/basecamp/portable");
    let _ = fs::remove_dir_all(&portable_dir);
    fs::create_dir_all(&portable_dir)
        .with_context(|| format!("create {}", portable_dir.display()))?;

    let mut outputs: Vec<PathBuf> = Vec::new();
    for (index, (name, src)) in ordered_names
        .iter()
        .zip(portable_sources.iter())
        .enumerate()
    {
        let store_paths: Vec<PathBuf> = match src {
            BasecampSource::Path(p) => vec![build_portable_resolve_path(Path::new(p))?],
            BasecampSource::Flake(flake_ref) => {
                // Sibling overrides still computed against the post-swap set
                // so path-sibling inputs resolve locally like they do at install.
                let overrides = resolve_sibling_overrides(src, &portable_sources, flake_ref);
                let inv = build_portable_nix_invocation(flake_ref, &overrides);
                run_build_portable_nix(&project.root, flake_ref, &inv)?
            }
        };

        // Symlink each store path into `portable_dir` with a load-ordered,
        // human-readable name. Two-digit index so a file-browser sorts the
        // list the same way the user should load them in basecamp.
        let load_order = format!("{:02}", index + 1);
        let multiple = store_paths.len() > 1;
        for store_path in &store_paths {
            let link_name = if multiple {
                let stem = store_path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("out");
                format!("{load_order}-{name}-{stem}.lgx")
            } else {
                format!("{load_order}-{name}.lgx")
            };
            let link_path = portable_dir.join(&link_name);
            std::os::unix::fs::symlink(store_path, &link_path).with_context(|| {
                format!(
                    "symlink {} -> {}",
                    link_path.display(),
                    store_path.display()
                )
            })?;
            outputs.push(link_path);
        }
    }

    println!(
        "Portable .lgx artefacts (in load order, symlinked into {}):",
        portable_dir.display()
    );
    for out in &outputs {
        println!("  {}", out.display());
    }
    Ok(())
}

/// Order `project_modules` so each module appears AFTER every
/// project-internal module it declares as a `metadata.json` dependency.
/// Modules with no project-internal deps come first, in alphabetical
/// order. Non-project deps (anything not keyed in `project_modules`) are
/// ignored — they're resolved at runtime, not load-time here.
///
/// Falls back to the remaining alphabetical order if a cycle is detected
/// (extremely unlikely in practice; a cycle would also mean the modules
/// can't load in any order and basecamp will error anyway — we just don't
/// want `build-portable` to hang).
fn topo_order_project_modules(
    project_root: &Path,
    project_modules: &std::collections::BTreeMap<String, ModuleEntry>,
) -> Vec<String> {
    let mut remaining: std::collections::BTreeMap<String, Vec<String>> = project_modules
        .iter()
        .map(|(name, entry)| {
            let src = module_entry_to_source(project_root, entry);
            let declared = read_source_metadata_dependencies(&src);
            let project_internal_deps: Vec<String> = declared
                .into_iter()
                .filter(|d| d != name && project_modules.contains_key(d))
                .collect();
            (name.clone(), project_internal_deps)
        })
        .collect();

    let mut out: Vec<String> = Vec::with_capacity(remaining.len());
    while !remaining.is_empty() {
        let mut ready: Vec<String> = remaining
            .iter()
            .filter(|(_, deps)| deps.is_empty())
            .map(|(k, _)| k.clone())
            .collect();
        ready.sort();

        if ready.is_empty() {
            // Cycle or unreachable state. Emit the rest alphabetically so
            // output stays deterministic; operator will see the load error
            // at basecamp runtime.
            let mut rest: Vec<String> = remaining.keys().cloned().collect();
            rest.sort();
            out.extend(rest);
            break;
        }

        for name in &ready {
            remaining.remove(name);
            out.push(name.clone());
        }
        for deps in remaining.values_mut() {
            deps.retain(|d| !ready.contains(d));
        }
    }
    out
}

/// Swap the attribute suffix on a flake ref: `path:/abs#lgx` → `path:/abs#lgx-portable`.
/// If the ref's attr differs from `expected`, the ref is returned unchanged
/// (trust the user; they explicitly pinned a non-default attr via
/// `basecamp modules --flake <ref>#<other>`).
fn swap_flake_attr(flake_ref: &str, expected: &str, replacement: &str) -> String {
    match flake_ref.rsplit_once('#') {
        Some((base, attr)) if attr == expected => format!("{base}#{replacement}"),
        Some((_, _)) => flake_ref.to_string(),
        None => flake_ref.to_string(),
    }
}

/// Command-line arguments (after `nix`) plus an optional working-directory
/// override. For `path:<abs>#<attr>` refs we cd into `<abs>` and invoke
/// `nix build .#<attr>` so the default `./result-<attr>` symlink lands next
/// to the flake. For remote refs we stay in the caller's cwd.
#[derive(Debug, PartialEq, Eq)]
struct NixBuildInvocation {
    cwd_override: Option<PathBuf>,
    args: Vec<String>,
}

fn build_portable_nix_invocation(
    flake_ref: &str,
    overrides: &[(String, String)],
) -> NixBuildInvocation {
    let (cwd_override, ref_arg) = match flake_path_prefix(flake_ref) {
        Some(abs) => {
            let attr = flake_ref
                .split_once('#')
                .map(|(_, a)| a)
                .unwrap_or("lgx-portable");
            (Some(PathBuf::from(abs)), format!(".#{attr}"))
        }
        None => (None, flake_ref.to_string()),
    };

    let mut args = vec!["build".to_string(), ref_arg];
    for (name, value) in overrides {
        args.push("--override-input".to_string());
        args.push(name.clone());
        args.push(value.clone());
    }
    args.push("--print-out-paths".to_string());

    NixBuildInvocation { cwd_override, args }
}

/// Validate a `--path` source and return its canonical absolute path.
fn build_portable_resolve_path(src: &Path) -> DynResult<PathBuf> {
    if !src.is_file() || !src.extension().is_some_and(|e| e == "lgx") {
        bail!("path `{}` is not a .lgx file", src.display());
    }
    Ok(src.canonicalize().unwrap_or_else(|_| src.to_path_buf()))
}

fn run_build_portable_nix(
    project_root: &Path,
    flake_ref: &str,
    inv: &NixBuildInvocation,
) -> DynResult<Vec<PathBuf>> {
    println!("building {flake_ref}");
    let mut cmd = Command::new("nix");
    match &inv.cwd_override {
        Some(cwd) => {
            cmd.current_dir(cwd);
        }
        None => {
            cmd.current_dir(project_root);
        }
    }
    for a in &inv.args {
        cmd.arg(a);
    }
    let output = cmd
        .output()
        .with_context(|| format!("spawn nix build {flake_ref}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if (stderr.contains("does not provide attribute") || stderr.contains("missing attribute"))
            && stderr.contains("lgx-portable")
        {
            bail!(
                "flake `{flake_ref}` does not expose `lgx-portable`. Either:\n\
                 (a) add a `packages.<system>.lgx-portable` output to your module's flake.nix, or\n\
                 (b) if you don't need a portable build, skip `basecamp build-portable` — \
                 `basecamp install` uses `.#lgx` and works without it.\n\
                 {COMPAT_DOCS_BREADCRUMB}"
            );
        }
        bail!(
            "nix build {flake_ref} failed ({}): {}",
            output.status,
            stderr.trim()
        );
    }
    // `nix build --print-out-paths` today emits only store paths on stdout;
    // diagnostics go to stderr. Accept only lines that look like absolute
    // filesystem paths so a future nix version that adds trailing summary
    // text to stdout doesn't pollute our output list. Users with non-standard
    // store prefixes (rare) still work — we don't hard-require `/nix/store/`.
    let stdout = String::from_utf8_lossy(&output.stdout);
    let store_paths: Vec<PathBuf> = stdout
        .lines()
        .map(|s| s.trim())
        .filter(|s| s.starts_with('/'))
        .map(PathBuf::from)
        .collect();
    if store_paths.is_empty() {
        bail!("nix build {flake_ref} returned no output paths");
    }
    let mut results = Vec::new();
    for sp in store_paths {
        if sp.is_dir() {
            results.extend(list_lgx(&sp)?);
        } else {
            results.push(sp);
        }
    }
    Ok(results)
}

/// Extract the rev / tag segment from a `github:owner/repo/<ref>#…` flake
/// ref. Returns `None` for non-github refs or refs without a ref segment.
/// Used by the doctor dep-pin drift row.
fn github_flake_ref_rev(flake_ref: &str) -> Option<&str> {
    let rest = flake_ref.strip_prefix("github:")?;
    let before_frag = rest.split_once('#').map_or(rest, |(b, _)| b);
    let parts: Vec<&str> = before_frag.split('/').collect();
    if parts.len() < 3 {
        return None;
    }
    Some(parts[2])
}

/// Given a source flake's local path and a dep name declared in its
/// `metadata.json`, try to find that name as an input in the sibling
/// `flake.lock` and return a `github:<owner>/<repo>/<rev>#lgx` ref.
///
/// Returns `None` if no local `flake.lock`, no matching input, or the
/// locked input isn't a github ref we can reconstruct. Non-fatal: callers
/// fall back to the next precedence layer.
fn resolve_dep_from_project_flake_lock(source_flake_path: &Path, dep_name: &str) -> Option<String> {
    let lock_path = source_flake_path.join("flake.lock");
    let text = fs::read_to_string(&lock_path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    let root = json.pointer("/nodes/root/inputs")?.as_object()?;
    // The root `inputs` object is a mapping of declared-input-name → node-id.
    // The node-id is typically the same as the input name but may be
    // suffixed with `_N` when multiple inputs share a name. Match on the
    // declared input name, not the node key.
    let node_id = root.get(dep_name)?.as_str()?;
    let locked = json.pointer(&format!("/nodes/{node_id}/locked"))?;
    let ty = locked.get("type")?.as_str()?;
    if ty != "github" {
        return None;
    }
    let owner = locked.get("owner")?.as_str()?;
    let repo = locked.get("repo")?.as_str()?;
    let rev = locked.get("rev")?.as_str()?;
    Some(format!("github:{owner}/{repo}/{rev}#lgx"))
}

/// Capture the project module set into `[basecamp.modules.*]` in
/// scaffold.toml. Sole writer of that section. `install` / `build-portable`
/// / `launch` only read it — they never discover on their own.
///
/// Three modes:
/// - `--show`: read-only; print the table grouped by role.
/// - Explicit (non-empty `paths` or `flakes`): captured set = exactly those
///   args as `role = "project"` entries.
/// - Auto (default, no args): walk project flakes; record as `role =
///   "project"`.
///
/// Dependency resolution from each source's `metadata.json.dependencies`
/// array lands in M2d and is intentionally absent here.
///
/// Idempotency: if a module_name already exists in `[basecamp.modules]`, its
/// entry is preserved — user intent (a hand-edited flake or role override)
/// wins over re-derivation.
fn cmd_basecamp_modules(
    mut project: Project,
    paths: Vec<PathBuf>,
    flakes: Vec<String>,
    show: bool,
    probe: &dyn LgxFlakeProbe,
) -> DynResult<()> {
    let existing_modules: std::collections::BTreeMap<String, ModuleEntry> =
        project.config.modules.clone();

    if show {
        print_modules_table("captured modules", &existing_modules);
        return Ok(());
    }

    let explicit = !paths.is_empty() || !flakes.is_empty();
    let flakes: Vec<String> = flakes
        .into_iter()
        .map(|f| normalize_flake_ref(&project.root, &f))
        .collect();

    let project_sources: Vec<BasecampSource> = if explicit {
        let mut out = Vec::new();
        for p in &paths {
            out.push(BasecampSource::Path(p.display().to_string()));
        }
        for f in &flakes {
            out.push(BasecampSource::Flake(f.clone()));
        }
        out
    } else {
        let cache_root_first = first_path_component(&project.config.cache_root);
        let mut skip_subdirs: Vec<&str> =
            BASECAMP_AUTODISCOVER_SKIP_SUBDIRS.iter().copied().collect();
        if let Some(c) = cache_root_first.as_deref() {
            if !skip_subdirs.contains(&c) {
                skip_subdirs.push(c);
            }
        }
        resolve_install_sources(
            &project.root,
            &[],
            &[],
            probe,
            &skip_subdirs,
            "lgx",
            "lgx-portable",
        )?
    };

    // Derive module_name per source, emit one stderr note per heuristic
    // guess, and insert into the table. Existing keys are left untouched.
    let mut new_modules = existing_modules.clone();
    for src in &project_sources {
        let (name, note) = derive_module_name(src)?;
        if let Some(n) = note {
            eprintln!("{}", assumption_note_line(&n));
        }
        new_modules.entry(name).or_insert_with(|| ModuleEntry {
            flake: relativize_flake_ref(&project.root, &flake_ref(src)),
            role: ModuleRole::Project,
        });
    }

    // Resolve manifest-declared runtime dependencies against the captured
    // set + scaffold defaults. Fails fast if any declared dep is unresolvable.
    let dep_entries = resolve_manifest_dependencies(&project_sources, &new_modules)?;
    for (name, entry) in dep_entries {
        new_modules.insert(name, entry);
    }

    // "previous modules (for reference)" block so reverting is copy-paste.
    print_modules_table("previous modules (for reference)", &existing_modules);

    project.config.modules = new_modules;
    save_project_config(&project)?;

    print_modules_table("captured modules", &project.config.modules);

    Ok(())
}

fn print_modules_table(header: &str, modules: &std::collections::BTreeMap<String, ModuleEntry>) {
    println!("{header}:");
    if modules.is_empty() {
        println!("  (none)");
        return;
    }
    let mut project_entries: Vec<_> = modules
        .iter()
        .filter(|(_, e)| e.role == ModuleRole::Project)
        .collect();
    let mut dep_entries: Vec<_> = modules
        .iter()
        .filter(|(_, e)| e.role == ModuleRole::Dependency)
        .collect();
    project_entries.sort_by_key(|(k, _)| k.as_str());
    dep_entries.sort_by_key(|(k, _)| k.as_str());

    println!("  project_sources:");
    if project_entries.is_empty() {
        println!("    (none)");
    } else {
        for (name, entry) in project_entries {
            println!("    {name} = {}", entry.flake);
        }
    }
    println!("  dependencies:");
    if dep_entries.is_empty() {
        println!("    (none)");
    } else {
        for (name, entry) in dep_entries {
            println!("    {name} = {}", entry.flake);
        }
    }
}

/// Captured at `basecamp modules` time when scaffold infers a `module_name`
/// for a source whose `metadata.json` wasn't readable locally (github flake,
/// `.lgx` file without a sibling manifest). Printed once to stderr via
/// `assumption_note_line`, then the inferred name is written to scaffold.toml
/// where the user can correct it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AssumptionNote {
    pub(crate) flake_ref: String,
    pub(crate) inferred_name: String,
}

/// Derive a `module_name` for a captured source. Returns `(name, Option<note>)`:
/// - `Some(note)` when the name had to be guessed (github repo slug, or
///   `.lgx` filename stem when no sibling `metadata.json` exists).
/// - `None` when the name was read directly from a `metadata.json.name` on
///   the local filesystem.
///
/// `path:` flakes without a readable `metadata.json` return an error, not a
/// fallback — the source directory is on disk and the file is part of the
/// module contract; a missing file is user-fixable, not guessable.
pub(crate) fn derive_module_name(
    src: &BasecampSource,
) -> DynResult<(String, Option<AssumptionNote>)> {
    match src {
        BasecampSource::Flake(flake_ref) => {
            if let Some(local) = flake_path_prefix(flake_ref) {
                let metadata_path = Path::new(local).join("metadata.json");
                let text = fs::read_to_string(&metadata_path).with_context(|| {
                    format!(
                        "read metadata.json for {flake_ref}: {} missing or unreadable",
                        metadata_path.display()
                    )
                })?;
                let json: serde_json::Value = serde_json::from_str(&text)
                    .with_context(|| format!("parse {}", metadata_path.display()))?;
                let raw = json.get("name").and_then(|v| v.as_str()).ok_or_else(|| {
                    anyhow::anyhow!(
                        "{} missing `name` field required for module identity",
                        metadata_path.display()
                    )
                })?;
                let name = normalize_and_validate_module_name(raw, &metadata_path.display())?;
                return Ok((name, None));
            }
            // Non-path flake (github:, git+, …) — guess from the ref itself.
            let raw = guess_name_from_github_ref(flake_ref).unwrap_or_else(|| {
                // Last-ditch: use whatever is after the final '/' before '#'
                let before_frag = flake_ref
                    .split_once('#')
                    .map_or(flake_ref.as_str(), |(b, _)| b);
                before_frag
                    .rsplit('/')
                    .next()
                    .unwrap_or(before_frag)
                    .replace('-', "_")
            });
            let inferred = normalize_and_validate_module_name(&raw, flake_ref)?;
            Ok((
                inferred.clone(),
                Some(AssumptionNote {
                    flake_ref: flake_ref.clone(),
                    inferred_name: inferred,
                }),
            ))
        }
        BasecampSource::Path(p) => {
            let pb = PathBuf::from(p);
            let sibling_metadata = pb.parent().map(|d| d.join("metadata.json"));
            if let Some(metadata_path) = &sibling_metadata {
                if let Ok(text) = fs::read_to_string(metadata_path) {
                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                        if let Some(raw) = json.get("name").and_then(|v| v.as_str()) {
                            let name =
                                normalize_and_validate_module_name(raw, &metadata_path.display())?;
                            return Ok((name, None));
                        }
                    }
                }
            }
            let raw = pb.file_stem().and_then(|s| s.to_str()).unwrap_or("unknown");
            let stem = normalize_and_validate_module_name(raw, p)?;
            Ok((
                stem.clone(),
                Some(AssumptionNote {
                    flake_ref: p.clone(),
                    inferred_name: stem,
                }),
            ))
        }
    }
}

/// Lowercase `raw` and confirm it matches the TOML bare-key charset scaffold
/// uses for `[basecamp.modules.<name>]` section headers — `[a-z0-9_-]` with a
/// non-dash first character. Guards against section-header injection through
/// `metadata.json`'s `name` or a github-slug derivation when that name flows
/// into the serialized scaffold.toml.
fn normalize_and_validate_module_name(
    raw: &str,
    source: &dyn std::fmt::Display,
) -> DynResult<String> {
    let normalized = raw.to_lowercase();
    let first_ok = normalized
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
    let rest_ok = normalized
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if !first_ok || !rest_ok {
        bail!(
            "invalid module name `{raw}` from {source}: must match [a-z0-9_-] with a \
             non-dash first character (case is normalized). Edit the source's metadata.json \
             `name` field, or hand-edit `[basecamp.modules]` in scaffold.toml."
        );
    }
    Ok(normalized)
}

/// Format the user-facing note printed once per heuristic guess. Names the
/// flake ref and the inferred `module_name`, and points at `scaffold.toml`
/// as the place to correct it.
pub(crate) fn assumption_note_line(note: &AssumptionNote) -> String {
    format!(
        "note: flake `{}` — assumed module_name = `{}`. If wrong, edit `[basecamp.modules]` in scaffold.toml.",
        note.flake_ref, note.inferred_name,
    )
}

/// Best-guess `module_name` from a `github:<owner>/<repo>[/<ref>][#<attr>]`
/// flake ref: repo slug, strip `logos-` prefix, replace `-` with `_`. Returns
/// `None` for non-github refs.
fn guess_name_from_github_ref(flake_ref: &str) -> Option<String> {
    let rest = flake_ref.strip_prefix("github:")?;
    let before_frag = rest.split_once('#').map_or(rest, |(b, _)| b);
    let repo = before_frag.split('/').nth(1)?;
    // Case-insensitive prefix strip so `Logos-Foo` and `logos-foo` normalize
    // the same way. `normalize_and_validate_module_name` lowercases after.
    let stripped = if repo.len() >= 6 && repo[..6].eq_ignore_ascii_case("logos-") {
        &repo[6..]
    } else {
        repo
    };
    Some(stripped.replace('-', "_"))
}

/// Read `metadata.json` from a flake source's local filesystem path and
/// collect its `dependencies: [...]` array. Returns empty for remote flakes
/// (no local path to read) or path-sources (`.lgx` files are build artefacts,
/// not source directories).
fn read_source_metadata_dependencies(src: &BasecampSource) -> Vec<String> {
    let BasecampSource::Flake(flake_ref) = src else {
        return Vec::new();
    };
    let Some(local_path) = flake_path_prefix(flake_ref) else {
        return Vec::new(); // github:, git+, http(s):, etc. — no local walk
    };
    let metadata_path = Path::new(local_path).join("metadata.json");
    let Ok(text) = fs::read_to_string(&metadata_path) else {
        return Vec::new();
    };
    let Ok(json): Result<serde_json::Value, _> = serde_json::from_str(&text) else {
        eprintln!(
            "warning: could not parse {} as JSON; skipping manifest deps",
            metadata_path.display()
        );
        return Vec::new();
    };
    json.get("dependencies")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// Resolve dependencies declared in project sources' `metadata.json` into
/// new `[basecamp.modules]` entries with `role = Dependency`. Returns only
/// the entries that need to be *added* to `captured` — names already in
/// `captured` (any role) are silently skipped.
///
/// Fails fast if any declared dep name can't be resolved through:
/// 1. an existing entry in `captured` (any role) — V-4 fix,
/// 2. the basecamp preinstall list,
/// 3. the declaring source's own `flake.lock` input of the same name,
/// 4. the scaffold-default `BASECAMP_DEPENDENCIES` pin table.
///
/// No warn-and-skip path: if nothing resolves, the capture is aborted with
/// a targeted error naming both user-side fixes.
fn resolve_manifest_dependencies(
    project_sources: &[BasecampSource],
    captured: &std::collections::BTreeMap<String, ModuleEntry>,
) -> DynResult<std::collections::BTreeMap<String, ModuleEntry>> {
    // Union of declared dep names across project sources, annotated with the
    // declaring source's flake ref (for error messages) and local path (used
    // to consult that source's own flake.lock).
    let mut declared: std::collections::BTreeMap<String, (String, Option<PathBuf>)> =
        std::collections::BTreeMap::new();
    for src in project_sources {
        let via = flake_ref(src);
        let local_path = match src {
            BasecampSource::Flake(f) => flake_path_prefix(f).map(PathBuf::from),
            BasecampSource::Path(_) => None,
        };
        for dep_name in read_source_metadata_dependencies(src) {
            declared
                .entry(dep_name)
                .or_insert_with(|| (via.clone(), local_path.clone()));
        }
    }

    let mut new_entries: std::collections::BTreeMap<String, ModuleEntry> =
        std::collections::BTreeMap::new();
    let mut unresolved: Vec<(String, String)> = Vec::new();

    for (name, (via, local_path)) in declared {
        if captured.contains_key(&name) {
            continue;
        }
        if BASECAMP_PREINSTALLED_MODULES.iter().any(|m| *m == name) {
            continue;
        }
        if let Some(path) = &local_path {
            if let Some(resolved) = resolve_dep_from_project_flake_lock(path, &name) {
                new_entries.insert(
                    name.clone(),
                    ModuleEntry {
                        flake: resolved,
                        role: ModuleRole::Dependency,
                    },
                );
                continue;
            }
        }
        if let Some((_, flake_ref)) = BASECAMP_DEPENDENCIES.iter().find(|(n, _)| *n == name) {
            new_entries.insert(
                name.clone(),
                ModuleEntry {
                    flake: (*flake_ref).to_string(),
                    role: ModuleRole::Dependency,
                },
            );
            continue;
        }
        unresolved.push((name, via));
    }

    if !unresolved.is_empty() {
        let mut msg = String::from(
            "unresolved module dependencies (declared in metadata.json but not resolvable):\n",
        );
        for (name, via) in &unresolved {
            msg.push_str(&format!("  - `{name}` declared by `{via}`\n"));
        }
        msg.push_str(
            "Either:\n  \
             (a) capture the module as a project source: `basecamp modules --flake <ref>#lgx`, or\n  \
             (b) add an explicit entry to scaffold.toml:\n      \
             [basecamp.modules.<name>]\n      \
             flake = \"<ref>#lgx\"\n      \
             role = \"dependency\"\n",
        );
        msg.push_str(COMPAT_DOCS_BREADCRUMB);
        msg.push('\n');
        bail!("{msg}");
    }

    Ok(new_entries)
}

/// Pure replay of `state.project_sources` + `state.dependencies` into every
/// seeded profile. Dependencies build first (fail-fast on a broken companion
/// pin, before we invest time on project sources). If state has no captured
/// sources, transparently invokes `basecamp modules` in auto-discover mode
/// and reloads state, then proceeds.
///
/// No CLI source flags (`--flake`/`--path`/`--profile`) — the source set
/// lives in state, managed by `basecamp modules`. No selective profile
/// installs either (KISS); install overwrites both profiles in one pass.
fn cmd_basecamp_install(project: Project, probe: &dyn LgxFlakeProbe) -> DynResult<()> {
    let state_path = project.root.join(".scaffold/state/basecamp.state");
    let state = read_basecamp_state(&state_path)
        .ok()
        .filter(|s| !s.basecamp_bin.is_empty() && !s.lgpm_bin.is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!("basecamp not set up yet; run: logos-scaffold basecamp setup")
        })?;

    if !Path::new(&state.basecamp_bin).exists() || !Path::new(&state.lgpm_bin).exists() {
        bail!("basecamp not set up yet; run: logos-scaffold basecamp setup");
    }

    // If scaffold.toml has no captured modules, run `modules` in
    // auto-discover mode transparently, then reload the project config.
    let project = if total_captured_modules(&project) == 0 {
        println!("no modules captured yet; running `basecamp modules` for you");
        cmd_basecamp_modules(project.clone(), Vec::new(), Vec::new(), false, probe)?;
        let cfg_text = fs::read_to_string(project.root.join("scaffold.toml"))
            .context("re-read scaffold.toml after auto-capture")?;
        let cfg = crate::config::parse_config(&cfg_text)?;
        Project {
            root: project.root,
            config: cfg,
        }
    } else {
        project
    };

    let (cache_root, _) = resolve_cache_root(&project)?;
    let lgx_cache = cache_root.join("basecamp/lgx-links");
    fs::create_dir_all(&lgx_cache).with_context(|| format!("create {}", lgx_cache.display()))?;

    let profiles_root = project.root.join(BASECAMP_PROFILES_REL);
    let target_profiles: Vec<String> = vec![
        BASECAMP_PROFILE_ALICE.to_string(),
        BASECAMP_PROFILE_BOB.to_string(),
    ];

    // Deps-first build order. fail-fast: a broken companion pin surfaces
    // before we invest nix build time on the dev's own modules.
    let ordered = captured_sources_deps_first(&project);
    if ordered.is_empty() {
        bail!(
            "no modules captured after auto-discovery; \
             run `basecamp modules --flake <ref>#lgx` to supply sources explicitly"
        );
    }

    println!("installing {} module(s) (deps first):", ordered.len());
    for src in &ordered {
        println!("  - {}", flake_ref(src));
    }

    let lgx_files = collect_lgx_files(&project.root, &ordered, &lgx_cache)?;
    if lgx_files.is_empty() {
        bail!("no .lgx files produced by the captured sources");
    }

    run_lgpm_install(
        &state.lgpm_bin,
        &profiles_root,
        &target_profiles,
        &lgx_files,
        true,
    )?;

    println!("install complete");
    Ok(())
}

/// Materialize every source into its `.lgx` files in the given cache dir. When
/// building a sub-flake, add `--override-input <sibling-dirname> path:<sibling>`
/// for every other path-flake sibling in `sources` so local edits in one
/// sub-flake flow into its dependents instead of pulling the sibling's master
/// branch from the network. Assumes the developer's layout matches flake input
/// names to sibling directory names (the scaffold convention for multi-flake
/// module repos).
fn collect_lgx_files(
    project_root: &Path,
    sources: &[BasecampSource],
    lgx_cache: &Path,
) -> DynResult<Vec<PathBuf>> {
    let mut out = Vec::new();
    for src in sources {
        let overrides = match src {
            BasecampSource::Flake(target_ref) => {
                resolve_sibling_overrides(src, sources, target_ref)
            }
            BasecampSource::Path(_) => Vec::new(),
        };
        out.extend(materialize_lgx_files(
            project_root,
            src,
            lgx_cache,
            &overrides,
        )?);
    }
    Ok(out)
}

/// Compute the sibling `--override-input` flags for a flake source, filtered
/// down to inputs the target flake actually declares. Shared by `install` and
/// `build-portable` so both commands stay in sync on the override rules and
/// the "skipped override" warning format.
///
/// On `flake_declared_inputs` failure (malformed flake, transient nix issue),
/// returns the unfiltered list — nix will warn on unknown inputs rather than
/// fail the build.
fn resolve_sibling_overrides(
    src: &BasecampSource,
    all: &[BasecampSource],
    target_ref: &str,
) -> Vec<(String, String)> {
    let mut overrides = sibling_overrides_for(src, all);
    if overrides.is_empty() {
        return overrides;
    }
    if let Ok(declared) = flake_declared_inputs(target_ref) {
        let before = overrides.clone();
        overrides = retain_declared_overrides(overrides, &declared);
        for (name, value) in &before {
            if !overrides.iter().any(|(n, _)| n == name) {
                eprintln!(
                    "warning: skipping sibling override `{name}` -> `{value}`: \
                     not declared as an input of `{target_ref}`"
                );
            }
        }
    }
    overrides
}

/// Drop overrides for input names that aren't in `declared`. Pure function so
/// tests can exercise the filter without shelling out to nix.
fn retain_declared_overrides(
    overrides: Vec<(String, String)>,
    declared: &std::collections::HashSet<String>,
) -> Vec<(String, String)> {
    overrides
        .into_iter()
        .filter(|(name, _)| declared.contains(name))
        .collect()
}

/// Names of the inputs declared by `flake_ref`'s root flake. Implemented via
/// `nix flake metadata --json` — lighter than `nix eval` since it only reads
/// the lockfile / flake.nix inputs block, not any package expressions.
fn flake_declared_inputs(flake_ref: &str) -> DynResult<std::collections::HashSet<String>> {
    // `nix flake metadata` rejects fragments (`path:/p#lgx` → "unexpected fragment
    // 'lgx' in flake reference"). Strip anything after `#` so we hand it just the
    // flake itself.
    let bare = flake_ref.split_once('#').map_or(flake_ref, |(b, _)| b);
    let out = Command::new("nix")
        .arg("flake")
        .arg("metadata")
        .arg("--json")
        .arg(bare)
        .output()
        .with_context(|| format!("spawn nix flake metadata {bare}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        bail!(
            "nix flake metadata {flake_ref} failed ({}): {}",
            out.status,
            stderr.trim()
        );
    }
    let json: serde_json::Value =
        serde_json::from_slice(&out.stdout).context("parse nix flake metadata JSON")?;
    let inputs = json
        .pointer("/locks/nodes/root/inputs")
        .and_then(|v| v.as_object())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    Ok(inputs)
}

/// Rewrite user-supplied plain-relative and absolute filesystem flake refs to
/// the `path:<abs>#attr` form the resolver and sibling-override logic expect.
/// Refs already carrying a scheme (`path:`, `github:`, `git+`, `http[s]:`,
/// `gitlab:`) pass through untouched.
///
/// Without this normalization, `--flake ./sub#lgx` would end up in the
/// resolver as-is; `sibling_overrides_for` only matches `path:` refs, so
/// sibling inputs silently wouldn't be auto-overridden, and `build-portable`'s
/// cwd-override would fall through to the project root instead of the flake
/// directory. Normalizing at the command boundary keeps the downstream
/// code paths uniform.
fn normalize_flake_ref(project_root: &Path, flake_ref: &str) -> String {
    if flake_ref.starts_with("github:")
        || flake_ref.starts_with("git+")
        || flake_ref.starts_with("http:")
        || flake_ref.starts_with("https:")
        || flake_ref.starts_with("gitlab:")
        || flake_ref.starts_with("sourcehut:")
    {
        return flake_ref.to_string();
    }
    // `path:./sub#attr` or `path:.#attr`: relative to project_root. Absolutize
    // so downstream consumers (sibling-override, nix build cwd, metadata read)
    // see a single uniform `path:<abs>#attr` shape regardless of whether the
    // ref was persisted in relative form.
    if let Some(rest) = flake_ref.strip_prefix("path:") {
        let (path_part, frag) = rest.split_once('#').unwrap_or((rest, ""));
        if path_part.starts_with('/') {
            return flake_ref.to_string();
        }
        let raw = project_root.join(path_part);
        let canon = raw.canonicalize().unwrap_or(raw);
        return if frag.is_empty() {
            format!("path:{}", canon.display())
        } else {
            format!("path:{}#{}", canon.display(), frag)
        };
    }
    let (path_part, frag) = flake_ref.split_once('#').unwrap_or((flake_ref, ""));
    let raw = if path_part.starts_with('/') {
        PathBuf::from(path_part)
    } else {
        project_root.join(path_part)
    };
    let canon = raw.canonicalize().unwrap_or(raw);
    if frag.is_empty() {
        format!("path:{}", canon.display())
    } else {
        format!("path:{}#{}", canon.display(), frag)
    }
}

/// Inverse of `normalize_flake_ref` for storage: if `flake_ref` is a
/// `path:<abs>` ref pointing inside `project_root`, rewrite it to the
/// relative `path:./<rel>` (or `path:.` for root) form so a committed
/// `scaffold.toml` stays portable across clones. Non-path refs and
/// out-of-project absolute paths pass through untouched.
fn relativize_flake_ref(project_root: &Path, flake_ref: &str) -> String {
    let Some(rest) = flake_ref.strip_prefix("path:") else {
        return flake_ref.to_string();
    };
    let (path_part, frag) = rest.split_once('#').unwrap_or((rest, ""));
    if !path_part.starts_with('/') {
        return flake_ref.to_string();
    }
    let abs = PathBuf::from(path_part);
    let canon_abs = abs.canonicalize().unwrap_or(abs);
    let canon_root = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    let Ok(rel) = canon_abs.strip_prefix(&canon_root) else {
        return flake_ref.to_string();
    };
    let rel_display = if rel.as_os_str().is_empty() {
        ".".to_string()
    } else {
        format!("./{}", rel.display())
    };
    if frag.is_empty() {
        format!("path:{rel_display}")
    } else {
        format!("path:{rel_display}#{frag}")
    }
}

/// Extract the absolute path from a `path:<abs>#attr` flake ref. Returns `None`
/// for non-path refs (`github:`, `git+`, etc.) — we can't safely override those
/// with a local path without user intent.
fn flake_path_prefix(flake_ref: &str) -> Option<&str> {
    let rest = flake_ref.strip_prefix("path:")?;
    let end = rest.find('#').unwrap_or(rest.len());
    Some(&rest[..end])
}

/// First path component of a relative subpath, e.g. `"cache"` for
/// `"cache/basecamp"`. Returns `None` if the path is empty or absolute.
fn first_path_component(rel: &str) -> Option<String> {
    Path::new(rel).components().find_map(|c| match c {
        Component::Normal(s) => s.to_str().map(|s| s.to_string()),
        _ => None,
    })
}

/// Compute the sibling-overrides list for `target`: collect every other
/// path-flake in `all` sharing `target`'s parent directory, then delegate to
/// `probe_sibling_overrides` to parse `target`'s own `flake.nix` and key
/// each override by the declared input name (not the sibling directory
/// name). This matters when the user's flake uses
/// `inputs.<snake>.url = "path:../<kebab>"` — the dir and input names
/// differ, and keying by dirname produces a nix `non-existent input`
/// warning plus a silently-unresolved original path.
fn sibling_overrides_for(target: &BasecampSource, all: &[BasecampSource]) -> Vec<(String, String)> {
    let BasecampSource::Flake(target_ref) = target else {
        return Vec::new();
    };
    let Some(target_abs) = flake_path_prefix(target_ref) else {
        return Vec::new();
    };
    let target_dir = Path::new(target_abs);
    let Some(target_parent) = target_dir.parent() else {
        return Vec::new();
    };

    let mut sibling_paths: Vec<PathBuf> = Vec::new();
    for other in all {
        let BasecampSource::Flake(other_ref) = other else {
            continue;
        };
        if other_ref == target_ref {
            continue;
        }
        let Some(other_abs) = flake_path_prefix(other_ref) else {
            continue;
        };
        let other_path = Path::new(other_abs);
        if other_path.parent() != Some(target_parent) {
            continue;
        }
        sibling_paths.push(other_path.to_path_buf());
    }

    let sibling_refs: Vec<&Path> = sibling_paths.iter().map(|p| p.as_path()).collect();
    let mut out = probe_sibling_overrides(target_dir, &sibling_refs);
    out.sort();
    out
}

/// `(modules_dir, plugins_dir)` under a profile's `XDG_DATA_HOME`. Pinning this
/// to one place keeps the layout knowledge from drifting between install/launch.
fn profile_modules_and_plugins(profiles_root: &Path, name: &str) -> (PathBuf, PathBuf) {
    let xdg_app = profiles_root
        .join(name)
        .join("xdg-data")
        .join(BASECAMP_XDG_APP_SUBPATH);
    (xdg_app.join("modules"), xdg_app.join("plugins"))
}

/// Install every lgx file into every target profile via lgpm. `announce = true`
/// prints per-file progress (install path); `false` is silent (launch replay).
fn run_lgpm_install(
    lgpm_bin: &str,
    profiles_root: &Path,
    profiles: &[String],
    lgx_files: &[PathBuf],
    announce: bool,
) -> DynResult<()> {
    for name in profiles {
        let (modules_dir, plugins_dir) = profile_modules_and_plugins(profiles_root, name);
        fs::create_dir_all(&modules_dir)
            .with_context(|| format!("create {}", modules_dir.display()))?;
        fs::create_dir_all(&plugins_dir)
            .with_context(|| format!("create {}", plugins_dir.display()))?;
        for lgx in lgx_files {
            if announce {
                println!("installing {} into {}", lgx.display(), name);
            }
            let args = lgpm_install_args(&modules_dir, &plugins_dir, lgx);
            run_checked(
                Command::new(lgpm_bin).args(&args),
                &format!("lgpm install {} into {}", lgx.display(), name),
            )?;
        }
    }
    Ok(())
}

/// Used by launch replay: build lgx files from the captured modules in
/// `[basecamp.modules]` and hand them to lgpm for the given profile(s).
/// No-op if no modules are captured. Dependencies build before project
/// sources so a broken companion pin surfaces before we invest nix build
/// time on the dev's own modules.
fn install_sources_into_profiles(
    project: &Project,
    state: &BasecampState,
    cache_root: &Path,
    profiles_root: &Path,
    profiles: &[String],
) -> DynResult<()> {
    let ordered = captured_sources_deps_first(project);
    if ordered.is_empty() {
        return Ok(());
    }
    let lgx_cache = cache_root.join("basecamp/lgx-links");
    fs::create_dir_all(&lgx_cache).with_context(|| format!("create {}", lgx_cache.display()))?;
    let lgx_files = collect_lgx_files(&project.root, &ordered, &lgx_cache)?;
    run_lgpm_install(&state.lgpm_bin, profiles_root, profiles, &lgx_files, false)
}

/// Build the argv (after the binary) for `lgpm install --file <lgx>` with the given
/// modules/plugins dirs. Lifted to a pure function so tests can pin the shape.
fn lgpm_install_args(
    modules_dir: &Path,
    plugins_dir: &Path,
    lgx: &Path,
) -> Vec<std::ffi::OsString> {
    vec![
        "--modules-dir".into(),
        modules_dir.as_os_str().to_owned(),
        "--ui-plugins-dir".into(),
        plugins_dir.as_os_str().to_owned(),
        "install".into(),
        "--file".into(),
        lgx.as_os_str().to_owned(),
    ]
}

/// Produce the list of `.lgx` files referenced by a given source.
/// For `Path`, expects a single `.lgx` file — directories are rejected.
/// For `Flake`, runs `nix build <ref>` and collects `*.lgx` at the build result root.
fn materialize_lgx_files(
    project_root: &Path,
    src: &BasecampSource,
    cache: &Path,
    overrides: &[(String, String)],
) -> DynResult<Vec<PathBuf>> {
    match src {
        BasecampSource::Path(p) => {
            let pb = PathBuf::from(p);
            if pb.is_file() && pb.extension().is_some_and(|e| e == "lgx") {
                return Ok(vec![pb]);
            }
            bail!("path `{}` is not a .lgx file", pb.display());
        }
        BasecampSource::Flake(flake_ref) => {
            let link = cache.join(flake_out_link_name(flake_ref));
            let mut cmd = Command::new("nix");
            cmd.arg("build").arg(flake_ref);
            for (name, value) in overrides {
                cmd.arg("--override-input").arg(name).arg(value);
            }
            cmd.arg("--out-link").arg(&link);
            let log = derive_log_path(project_root, "install");
            run_logged(&mut cmd, &format!("building {flake_ref}"), &log)?;
            list_lgx(&link)
        }
    }
}

/// Out-link filename for a user-supplied flake ref. Slugified for readability, with a
/// short hash suffix so two refs that slugify the same don't clobber each other's build.
/// Uses FNV-1a 64-bit so the suffix is deterministic across Rust versions — unlike
/// `DefaultHasher`, which may rehash differently between releases and invalidate
/// persisted `result` symlinks under the nix store after a compiler bump.
fn flake_out_link_name(flake_ref: &str) -> String {
    let slug: String = flake_ref
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let hash = fnv1a_64(flake_ref.as_bytes());
    format!("{slug}-{hash:016x}-result")
}

/// FNV-1a 64-bit. Stable, dependency-free, good enough for collision-avoidance
/// on short slugs. Not cryptographic.
fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01B3);
    }
    h
}

fn list_lgx(dir: &Path) -> DynResult<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let entry = entry.with_context(|| format!("read entry in {}", dir.display()))?;
        let path = entry.path();
        if path.is_file() && path.extension().is_some_and(|e| e == "lgx") {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

struct NixLgxProbe;

impl LgxFlakeProbe for NixLgxProbe {
    fn package_names(
        &self,
        flake_ref: &str,
        overrides: &[(String, String)],
    ) -> DynResult<Vec<String>> {
        let target = format!("{flake_ref}#packages.{}", nix_current_system());
        let mut cmd = Command::new("nix");
        cmd.arg("eval")
            .arg("--json")
            .arg("--apply")
            .arg("x: builtins.attrNames x")
            .arg(&target);
        for (name, value) in overrides {
            cmd.arg("--override-input").arg(name).arg(value);
        }
        let out = cmd
            .output()
            .with_context(|| format!("spawn nix eval {target}"))?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            // A flake with no `packages.<system>` attribute is a normal resolver case
            // (e.g., a project whose flake only exposes devShells). Treat as empty.
            // Anything else — lockfile errors, syntax errors, network failures —
            // must propagate so the user sees the real reason instead of the generic
            // "no .lgx sources found" fallback.
            if stderr.contains("does not provide attribute") || stderr.contains("missing attribute")
            {
                return Ok(Vec::new());
            }
            bail!(
                "nix eval {target} failed ({}): {}\n{}",
                out.status,
                stderr.trim(),
                COMPAT_DOCS_BREADCRUMB,
            );
        }
        let text = String::from_utf8(out.stdout).context("nix eval output not utf-8")?;
        let names: Vec<String> =
            serde_json::from_str(text.trim()).context("parse nix eval JSON")?;
        Ok(names)
    }
}

fn nix_current_system() -> &'static str {
    if cfg!(target_os = "macos") {
        if cfg!(target_arch = "aarch64") {
            "aarch64-darwin"
        } else {
            "x86_64-darwin"
        }
    } else if cfg!(target_arch = "aarch64") {
        "aarch64-linux"
    } else {
        "x86_64-linux"
    }
}

/// Map the current host's nix system string to the `.lgx` manifest `main`
/// variant key basecamp resolves against when loading a dev-build plugin.
/// Returns `None` for platforms we can't map (unknown nix systems).
pub(crate) fn platform_dev_variant_key() -> Option<&'static str> {
    match nix_current_system() {
        "x86_64-linux" => Some("linux-amd64-dev"),
        "aarch64-linux" => Some("linux-arm64-dev"),
        "aarch64-darwin" => Some("darwin-arm64-dev"),
        "x86_64-darwin" => Some("darwin-amd64-dev"),
        _ => None,
    }
}

/// A `modules/<name>/manifest.json` whose `main` object lacks the expected
/// dev-variant key for the current platform. Basecamp v0.1.1's variant
/// resolver will silently hang on first click when loading such a plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ManifestVariantIssue {
    pub(crate) profile: String,
    pub(crate) module_name: String,
    pub(crate) available_variants: Vec<String>,
}

/// Count installed modules under `<profile_dir>/xdg-data/<BASECAMP_XDG_APP_SUBPATH>/modules/`.
/// Returns 0 if the dir doesn't exist yet.
pub(crate) fn count_installed_modules(profile_dir: &Path) -> usize {
    let modules_root = profile_dir
        .join("xdg-data")
        .join(BASECAMP_XDG_APP_SUBPATH)
        .join("modules");
    let Ok(entries) = fs::read_dir(&modules_root) else {
        return 0;
    };
    entries.flatten().filter(|e| e.path().is_dir()).count()
}

/// Walk `<profile_dir>/xdg-data/<BASECAMP_XDG_APP_SUBPATH>/modules/*/manifest.json`
/// and flag modules missing the expected `-dev` variant key. Non-blocking:
/// callers decide whether to warn or fail based on the returned issues.
/// Silent on parse errors / missing files (not the check's job to police).
pub(crate) fn check_manifest_variants(
    profile_dir: &Path,
    profile_name: &str,
    expected_dev_variant: &str,
) -> Vec<ManifestVariantIssue> {
    let modules_root = profile_dir
        .join("xdg-data")
        .join(BASECAMP_XDG_APP_SUBPATH)
        .join("modules");
    let Ok(entries) = fs::read_dir(&modules_root) else {
        return Vec::new();
    };
    let mut issues = Vec::new();
    for entry in entries.flatten() {
        let module_dir = entry.path();
        if !module_dir.is_dir() {
            continue;
        }
        let manifest_path = module_dir.join("manifest.json");
        let Ok(text) = fs::read_to_string(&manifest_path) else {
            continue;
        };
        let Ok(json): Result<serde_json::Value, _> = serde_json::from_str(&text) else {
            continue;
        };
        let name = json
            .get("name")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| {
                module_dir
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("<unknown>")
                    .to_string()
            });
        let main = json.get("main");
        let available: Vec<String> = match main {
            Some(serde_json::Value::Object(m)) => m.keys().cloned().collect(),
            // `main` might be a plain string (single-platform modules) —
            // not variant-keyed, can't reason about -dev presence.
            _ => continue,
        };
        if !available.is_empty() && !available.iter().any(|k| k.as_str() == expected_dev_variant) {
            issues.push(ManifestVariantIssue {
                profile: profile_name.to_string(),
                module_name: name,
                available_variants: available,
            });
        }
    }
    issues
}

/// Sources `basecamp modules` would auto-discover today but which aren't in
/// `state.sources`. Used by `doctor` to flag missing captures without running
/// the real capture. The reverse direction (captured-but-not-discovered) is
/// intentionally not reported: explicitly captured sources (e.g. `--path` to
/// a nix store, a github ref outside the project tree) are never
/// auto-discoverable, and flagging them produced noisy false positives.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ModuleDriftReport {
    pub(crate) discovered_not_captured: Vec<BasecampSource>,
}

/// Run `basecamp modules` auto-discovery as a dry-run and diff the project
/// sources it would find against `[basecamp.modules]` entries with
/// `role = "project"`. Dependencies are not considered here — if a project
/// source is captured, `basecamp modules` resolves its deps deterministically
/// at capture time, so any dep drift surfaces via the modules command itself
/// rather than via this doctor row.
pub(crate) fn compute_module_drift(project: &Project) -> DynResult<ModuleDriftReport> {
    let cache_root_first = first_path_component(&project.config.cache_root);
    let mut skip_subdirs: Vec<&str> = BASECAMP_AUTODISCOVER_SKIP_SUBDIRS.iter().copied().collect();
    if let Some(c) = cache_root_first.as_deref() {
        if !skip_subdirs.contains(&c) {
            skip_subdirs.push(c);
        }
    }

    let probe = NixLgxProbe;
    let discovered_project = resolve_install_sources(
        &project.root,
        &[],
        &[],
        &probe,
        &skip_subdirs,
        "lgx",
        "lgx-portable",
    )
    .unwrap_or_default();

    let captured_flakes: std::collections::HashSet<String> = project
        .config
        .modules
        .values()
        .filter(|e| e.role == ModuleRole::Project)
        .map(|e| e.flake.clone())
        .collect();

    let mut discovered_not_captured: Vec<BasecampSource> = discovered_project
        .into_iter()
        .filter(|src| !captured_flakes.contains(&flake_ref(src)))
        .collect();
    discovered_not_captured.sort_by_key(flake_ref);

    Ok(ModuleDriftReport {
        discovered_not_captured,
    })
}

/// `basecamp doctor` entry point. Builds a scaffold `DoctorReport` containing
/// only basecamp-specific rows (captured modules summary, manifest variant
/// check, module-set drift) and prints/serializes it via the shared doctor
/// formatting helpers.
///
/// Intentionally separate from top-level `logos-scaffold doctor` so basecamp
/// remains a self-contained subcommand surface. A follow-up PR may merge the
/// rows into the global doctor once the core basecamp feature has landed.
fn cmd_basecamp_doctor(project: Project, as_json: bool) -> DynResult<()> {
    use crate::commands::doctor::{finalize_report, print_report};
    use crate::model::CheckStatus;

    if as_json {
        crate::process::set_command_echo(false);
    }

    let mut rows = Vec::new();
    push_basecamp_doctor_rows(&project, &mut rows);

    // If there are no rows at all, state is absent or empty — emit a single
    // Pass row so the output is never confusingly blank.
    if rows.is_empty() {
        rows.push(crate::model::CheckRow {
            status: CheckStatus::Pass,
            name: "basecamp state".to_string(),
            detail: "not set up yet (no basecamp.state)".to_string(),
            remediation: Some("run `logos-scaffold basecamp setup`".to_string()),
        });
    }

    let report = finalize_report(rows);

    if as_json {
        crate::process::set_command_echo(true);
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_report(&report);
    }

    if report.summary.fail > 0 {
        bail!("basecamp doctor reported FAIL checks");
    }
    Ok(())
}

/// Build the basecamp-specific doctor rows: captured modules summary (each
/// source's flake ref + commit/tag + installed api headers), manifest variant
/// checks per seeded profile, and a missing-module drift check against
/// auto-discovery. No-op when neither `basecamp.state` nor
/// `[basecamp.modules]` carries anything.
fn push_basecamp_doctor_rows(project: &Project, rows: &mut Vec<crate::model::CheckRow>) {
    use crate::model::{CheckRow, CheckStatus};

    let state_path = project.root.join(".scaffold/state/basecamp.state");
    let state = read_basecamp_state(&state_path).unwrap_or_default();
    let module_count = total_captured_modules(project);
    if state.basecamp_bin.is_empty() && state.lgpm_bin.is_empty() && module_count == 0 {
        return;
    }

    let profiles_root = project.root.join(BASECAMP_PROFILES_REL);
    let alice_modules = profiles_root
        .join(BASECAMP_PROFILE_ALICE)
        .join("xdg-data")
        .join(BASECAMP_XDG_APP_SUBPATH)
        .join("modules");

    {
        let mut entries: Vec<(&String, &ModuleEntry)> = project.config.modules.iter().collect();
        entries.sort_by_key(|(_, e)| e.role == ModuleRole::Dependency);
        for (_name, entry) in entries {
            let label = match entry.role {
                ModuleRole::Project => "basecamp module",
                ModuleRole::Dependency => "basecamp dep",
            };
            let src = module_entry_to_source(&project.root, entry);
            rows.push(captured_source_row(label, &src, &alice_modules));
        }
    }

    // Dep-pin drift: captured `role = Dependency` entry rev differs from the
    // scaffold default for the same module_name. Exact-match lookup — no
    // repo-slug heuristic (R-I3 fix).
    for (name, default_ref) in BASECAMP_DEPENDENCIES {
        let Some(entry) = project.config.modules.get(*name) else {
            continue;
        };
        if entry.role != ModuleRole::Dependency {
            continue;
        }
        let captured_rev = github_flake_ref_rev(&entry.flake);
        let default_rev = github_flake_ref_rev(default_ref);
        let drifted = match (captured_rev, default_rev) {
            (Some(c), Some(d)) => c != d,
            _ => entry.flake.as_str() != *default_ref,
        };
        if drifted {
            rows.push(CheckRow {
                status: CheckStatus::Warn,
                name: format!("basecamp dep pin drift: {name}"),
                detail: format!(
                    "captured `{}` differs from scaffold default `{}`",
                    captured_rev.unwrap_or(&entry.flake),
                    default_rev.unwrap_or(default_ref),
                ),
                remediation: Some(format!(
                    "module may not work against a basecamp release built with the scaffold \
                     default. Update `[modules.{name}].flake` in scaffold.toml to \
                     a compatible rev."
                )),
            });
        }
    }

    if let Some(expected_dev) = platform_dev_variant_key() {
        for profile in [BASECAMP_PROFILE_ALICE, BASECAMP_PROFILE_BOB] {
            let profile_dir = profiles_root.join(profile);
            if !profile_dir.is_dir() {
                continue;
            }
            for issue in check_manifest_variants(&profile_dir, profile, expected_dev) {
                rows.push(CheckRow {
                    status: CheckStatus::Warn,
                    name: format!(
                        "basecamp variant: {} in profile {}",
                        issue.module_name, issue.profile
                    ),
                    detail: format!(
                        "main=[{}] missing expected `{}`; plugin will hang on click",
                        issue.available_variants.join(","),
                        expected_dev
                    ),
                    remediation: Some(format!(
                        "rebuild `{}` so its manifest.json `main.{}` key is populated \
                         (upstream logos-module-builder issue); then re-run `basecamp install`",
                        issue.module_name, expected_dev
                    )),
                });
            }
        }
    }

    if let Ok(drift) = compute_module_drift(project) {
        for src in &drift.discovered_not_captured {
            rows.push(CheckRow {
                status: CheckStatus::Warn,
                name: "basecamp drift: uncaptured".to_string(),
                detail: format!(
                    "discovered `{}` but not captured in basecamp.state",
                    flake_ref(src)
                ),
                remediation: Some(
                    "run `logos-scaffold basecamp modules` to refresh capture".to_string(),
                ),
            });
        }
    }
}

/// One doctor row per captured source. Shows the flake ref verbatim plus a
/// tag/commit annotation for github refs and any `*.h` / `*.hpp` headers
/// already installed under alice's profile.
fn captured_source_row(
    label: &str,
    src: &BasecampSource,
    alice_modules: &Path,
) -> crate::model::CheckRow {
    use crate::model::{CheckRow, CheckStatus};
    let ref_text = flake_ref(src);
    let mut detail = ref_text.clone();

    if let BasecampSource::Flake(flake_ref) = src {
        if let Some(label) = github_ref_part_label(flake_ref) {
            detail.push_str(&format!("  ({label})"));
        }
        if let Some(module_name) = infer_module_name_from_flake_ref(flake_ref) {
            let headers = collect_api_headers(alice_modules, &module_name);
            if !headers.is_empty() {
                detail.push_str(&format!(
                    "\n    api headers: {}",
                    headers
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
        }
    }

    CheckRow {
        status: CheckStatus::Pass,
        name: label.to_string(),
        detail,
        remediation: None,
    }
}

fn github_ref_part_label(flake_ref: &str) -> Option<String> {
    let rest = flake_ref.strip_prefix("github:")?;
    let before_frag = rest.split_once('#').map_or(rest, |(b, _)| b);
    let parts: Vec<&str> = before_frag.split('/').collect();
    if parts.len() < 3 {
        return None;
    }
    let ref_part = parts[2];
    let looks_like_commit = ref_part.len() >= 7
        && ref_part.len() <= 40
        && ref_part.chars().all(|c| c.is_ascii_hexdigit());
    if looks_like_commit {
        Some(format!("commit {}", &ref_part[..ref_part.len().min(12)]))
    } else {
        Some(format!("tag {ref_part}"))
    }
}

fn infer_module_name_from_flake_ref(flake_ref: &str) -> Option<String> {
    let before_frag = flake_ref.split_once('#').map_or(flake_ref, |(b, _)| b);
    if let Some(rest) = before_frag.strip_prefix("github:") {
        let repo = rest.split('/').nth(1)?;
        let trimmed = repo.trim_start_matches("logos-");
        return Some(trimmed.replace('-', "_"));
    }
    if let Some(rest) = before_frag.strip_prefix("path:") {
        return Some(
            Path::new(rest)
                .file_name()?
                .to_string_lossy()
                .replace('-', "_"),
        );
    }
    None
}

fn collect_api_headers(alice_modules: &Path, module_name: &str) -> Vec<PathBuf> {
    let module_dir = alice_modules.join(module_name);
    if !module_dir.is_dir() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let candidates = [
        module_dir.clone(),
        module_dir.join("include"),
        module_dir.join("interfaces"),
    ];
    for dir in candidates {
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
                        if ext.eq_ignore_ascii_case("h") || ext.eq_ignore_ascii_case("hpp") {
                            out.push(path);
                        }
                    }
                }
            }
        }
    }
    out.sort();
    out
}

/// Probes a flake ref for the set of package names it exposes under the current
/// system. Returned list is used to detect `.#lgx` / `.#lgx-portable` in source
/// resolution.
///
/// `overrides` is the list of `(input_name, flake_ref)` pairs to pass as
/// `--override-input` to `nix eval`. Sub-flakes that declare inputs of the form
/// `path:../<sibling>` cannot be evaluated in pure mode without those overrides
/// — nix copies the flake into the store before evaluating, and `..` then
/// resolves under `/nix/store/` rather than the real project tree. Mirrors the
/// sibling-override plumbing already applied at build time.
trait LgxFlakeProbe {
    fn package_names(
        &self,
        flake_ref: &str,
        overrides: &[(String, String)],
    ) -> DynResult<Vec<String>>;
}

/// Resolves the set of `.lgx` sources to install from explicit args and project auto-discovery.
///
/// Precedence (matches §2.2 of the basecamp-profiles spec):
/// 1. Explicit `--path` / `--flake` — wins if supplied.
/// 2. Project root `flake.nix` exposing `packages.<system>.<attr>` — build that attribute.
/// 3. Sub-directories with a `flake.nix` exposing the same.
/// 4. Project exposes only `.#<alt_attr>` — fail with a targeted hint pointing at it.
/// 5. No matching sources found anywhere — fail with a generic hint.
///
/// `attr` selects which flake output to target (`"lgx"` for `install`, `"lgx-portable"`
/// for `build-portable`). `alt_attr` is the variant reported in the fail-hint when only
/// the other one is present.
fn resolve_install_sources(
    project_root: &Path,
    explicit_paths: &[PathBuf],
    explicit_flakes: &[String],
    probe: &dyn LgxFlakeProbe,
    skip_subdirs: &[&str],
    attr: &str,
    alt_attr: &str,
) -> DynResult<Vec<BasecampSource>> {
    if !explicit_paths.is_empty() || !explicit_flakes.is_empty() {
        let mut out = Vec::new();
        for p in explicit_paths {
            out.push(BasecampSource::Path(p.display().to_string()));
        }
        for f in explicit_flakes {
            out.push(BasecampSource::Flake(f.clone()));
        }
        return Ok(out);
    }

    let mut found = Vec::new();
    let mut alt_only_dirs = Vec::new();
    let root_flake = project_root.join("flake.nix");
    if root_flake.is_file() {
        // Root flake: no sibling overrides. The root's `path:./sub` inputs
        // stay inside the store copy; sibling-of-parent resolution doesn't
        // apply at this level.
        classify_flake_dir(
            project_root,
            probe,
            attr,
            alt_attr,
            &[],
            &mut found,
            &mut alt_only_dirs,
        )?;
    }

    if found.is_empty() {
        // Collect every flake-bearing sub-directory first so each probe call
        // knows its filesystem siblings (the other sub-flakes). Probing
        // `sub-a/flake.nix` with an input like `path:../sub-b` needs
        // `--override-input sub-b path:<abs>/sub-b`, otherwise nix copies
        // sub-a into the store and `..` resolves outside the store copy —
        // pure-eval rejects it.
        let mut sub_dirs: Vec<PathBuf> = Vec::new();
        for entry in fs::read_dir(project_root)
            .with_context(|| format!("read {}", project_root.display()))?
        {
            let entry =
                entry.with_context(|| format!("read entry in {}", project_root.display()))?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            if name.starts_with('.') || skip_subdirs.iter().any(|s| *s == name) {
                continue;
            }
            if path.join("flake.nix").is_file() {
                sub_dirs.push(path);
            }
        }
        sub_dirs.sort();

        for dir in &sub_dirs {
            let siblings: Vec<&Path> = sub_dirs
                .iter()
                .filter(|&d| d != dir)
                .map(|p| p.as_path())
                .collect();
            classify_flake_dir(
                dir,
                probe,
                attr,
                alt_attr,
                &siblings,
                &mut found,
                &mut alt_only_dirs,
            )?;
        }
    }

    if !found.is_empty() {
        found.sort_by_key(flake_ref);
        return Ok(found);
    }

    if !alt_only_dirs.is_empty() {
        alt_only_dirs.sort();
        bail!(
            "found `.#{alt_attr}` in {dirs} but no `.#{attr}` output.\n\
             Next step: expose `packages.<system>.{attr}` in your flake, or re-run with \
             `--flake <path>#{alt_attr}` to opt into the {alt_attr} variant explicitly.\n\
             {COMPAT_DOCS_BREADCRUMB}",
            dirs = alt_only_dirs.join(", "),
        );
    }

    bail!(
        "no `.lgx` sources found in this project.\n\
         Next step: expose `packages.<system>.{attr}` in a `flake.nix` at the project root \
         (or in a sub-directory), or pass `--path <file.lgx>` / `--flake <ref>#{attr}`.\n\
         {COMPAT_DOCS_BREADCRUMB}"
    )
}

fn classify_flake_dir(
    dir: &Path,
    probe: &dyn LgxFlakeProbe,
    attr: &str,
    alt_attr: &str,
    siblings: &[&Path],
    found: &mut Vec<BasecampSource>,
    alt_only_dirs: &mut Vec<String>,
) -> DynResult<()> {
    debug_assert_ne!(
        attr, alt_attr,
        "classify_flake_dir requires distinct attr / alt_attr — the `else if` \
         would be unreachable and alt-only detection silently broken"
    );
    let flake_ref = format!("path:{}", dir.display());
    let overrides = probe_sibling_overrides(dir, siblings);
    let names = probe.package_names(&flake_ref, &overrides)?;
    // A flake that exposes only `alt_attr` (not the requested `attr`) is a failure case
    // handled by the caller with a targeted hint.
    if names.iter().any(|n| n == attr) {
        found.push(BasecampSource::Flake(format!("{flake_ref}#{attr}")));
    } else if names.iter().any(|n| n == alt_attr) {
        alt_only_dirs.push(dir.display().to_string());
    }
    Ok(())
}

/// Build `--override-input <input_name> path:<abs-sibling>` pairs by reading
/// `<target_dir>/flake.nix` for `path:../<sibling>` inputs and matching each
/// URL target against the sibling directories on disk.
///
/// We can't key overrides by the sibling's directory name because the sub-
/// flake's declared input name can differ (e.g. input `tictactoe_solo_ai`
/// referencing dir `logos-tictactoe-solo-ai`). Passing the override under the
/// wrong key emits a nix warning ("input has an override for a non-existent
/// input") and silently falls through to the original `path:..` URL — which
/// then fails pure-eval. So we parse flake.nix for the input name.
///
/// Parsing is best-effort line-level regex-lite. Matches `<name>.url = "…"`
/// (with or without an `inputs.` prefix) and the value starting with
/// `path:../`. Nix syntax it won't catch (multi-line values, let-bindings,
/// `inputs = { x = { url = …; }; }` nested blocks written on separate lines
/// from `x`) falls through and the probe may still fail — but the common
/// declarative form works.
fn probe_sibling_overrides(target_dir: &Path, siblings: &[&Path]) -> Vec<(String, String)> {
    let flake_nix = target_dir.join("flake.nix");
    let Ok(text) = fs::read_to_string(&flake_nix) else {
        return Vec::new();
    };
    let parsed = parse_path_dotdot_inputs(&text);
    if parsed.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (input_name, sibling_stem) in parsed {
        let Some(sibling_path) = siblings.iter().find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n == sibling_stem)
        }) else {
            continue;
        };
        let abs = sibling_path
            .canonicalize()
            .unwrap_or_else(|_| sibling_path.to_path_buf());
        out.push((input_name, format!("path:{}", abs.display())));
    }
    out
}

/// Parse lines of the form `<name>.url = "path:../<sibling>"` out of a
/// flake.nix text. Returns `(input_name, sibling_dir_stem)` pairs.
///
/// Deliberately permissive — matches both `inputs.foo.url = "…"` and bare
/// `foo.url = "…"` (the common form inside an `inputs = { … }` block).
/// The path: scheme must be present and start with `../`; anything else
/// (`github:`, `path:./sub`, absolute paths) is ignored as out-of-scope for
/// sibling-override resolution.
fn parse_path_dotdot_inputs(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some(url_at) = line.find(".url") else {
            continue;
        };
        let before_url = &line[..url_at];
        // Input name is the last identifier-ish token in `before_url`.
        let name = before_url
            .rsplit(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-')
            .next()
            .unwrap_or("");
        if name.is_empty() || name == "inputs" {
            continue;
        }
        let after_url = &line[url_at + ".url".len()..];
        let Some(eq_at) = after_url.find('=') else {
            continue;
        };
        let Some(value) = extract_first_string_literal(&after_url[eq_at + 1..]) else {
            continue;
        };
        let Some(rest) = value.strip_prefix("path:../") else {
            continue;
        };
        let sibling = rest.split(['/', '?', '#']).next().unwrap_or("");
        if sibling.is_empty() {
            continue;
        }
        out.push((name.to_string(), sibling.to_string()));
    }
    out
}

/// Extract the first double-quoted string literal from `s`. Returns the
/// string's inner contents (no surrounding quotes). Doesn't handle escaped
/// quotes — good enough for URL values, which don't carry `\"` in practice.
fn extract_first_string_literal(s: &str) -> Option<String> {
    let start = s.find('"')? + 1;
    let rest = &s[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

pub(crate) fn flake_ref(src: &BasecampSource) -> String {
    match src {
        BasecampSource::Path(p) => p.clone(),
        BasecampSource::Flake(f) => f.clone(),
    }
}

/// Recover a `BasecampSource` from a stored `ModuleEntry.flake` string. A
/// captured `--path <file.lgx>` was stored as its raw path with no `path:`
/// prefix; everything else came from `flake_ref` on a `BasecampSource::Flake`.
/// The `.lgx` suffix is the discriminator — path-sources are always `.lgx`
/// files by construction (the CLI validates this on capture).
///
/// Flake refs are run through `normalize_flake_ref` so relative `path:./sub`
/// values persisted in scaffold.toml are re-anchored to `project_root` before
/// downstream consumers (nix build cwd, sibling override, metadata read) see
/// them.
fn module_entry_to_source(project_root: &Path, entry: &ModuleEntry) -> BasecampSource {
    if entry.flake.ends_with(".lgx") && !entry.flake.contains('#') {
        BasecampSource::Path(entry.flake.clone())
    } else {
        BasecampSource::Flake(normalize_flake_ref(project_root, &entry.flake))
    }
}

/// Convert captured modules in `[basecamp.modules]` filtered by `role` into
/// a `Vec<BasecampSource>` in key order. Key order is BTreeMap-sorted, which
/// happens to give deps-first semantics only by coincidence — callers that
/// need deps-first explicitly should iterate by role.
fn project_role_sources(project: &Project, role: ModuleRole) -> Vec<BasecampSource> {
    project
        .config
        .modules
        .values()
        .filter(|e| e.role == role)
        .map(|e| module_entry_to_source(&project.root, e))
        .collect()
}

/// Return all captured sources with `role = Dependency` first, then
/// `role = Project`. Install's existing invariant is "deps before project
/// sources" so a broken companion pin fails fast before any project build.
fn captured_sources_deps_first(project: &Project) -> Vec<BasecampSource> {
    let mut deps = project_role_sources(project, ModuleRole::Dependency);
    deps.extend(project_role_sources(project, ModuleRole::Project));
    deps
}

fn total_captured_modules(project: &Project) -> usize {
    project.config.modules.len()
}

/// Create XDG-rooted profile dirs under `profiles_root` for every named profile.
/// Returns the list of profile names that now exist (idempotent).
fn seed_profiles(profiles_root: &Path, names: &[&str]) -> DynResult<Vec<String>> {
    let mut seeded = Vec::new();
    for name in names {
        let profile_dir = profiles_root.join(name);
        for xdg in ["xdg-config", "xdg-data", "xdg-cache"] {
            let path = profile_dir.join(xdg).join(BASECAMP_XDG_APP_SUBPATH);
            fs::create_dir_all(&path)
                .with_context(|| format!("create profile dir {}", path.display()))?;
        }
        seeded.push(name.to_string());
    }
    Ok(seeded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use tempfile::tempdir;

    struct FakeProbe {
        answers: RefCell<HashMap<String, Vec<String>>>,
    }

    impl FakeProbe {
        fn new(answers: &[(&str, &[&str])]) -> Self {
            let mut map = HashMap::new();
            for (k, v) in answers {
                map.insert(k.to_string(), v.iter().map(|s| s.to_string()).collect());
            }
            Self {
                answers: RefCell::new(map),
            }
        }
    }

    impl LgxFlakeProbe for FakeProbe {
        fn package_names(
            &self,
            flake_ref: &str,
            _overrides: &[(String, String)],
        ) -> DynResult<Vec<String>> {
            Ok(self
                .answers
                .borrow()
                .get(flake_ref)
                .cloned()
                .unwrap_or_default())
        }
    }

    #[test]
    fn resolve_install_sources_explicit_paths_win() {
        let tmp = tempdir().expect("tempdir");
        let probe = FakeProbe::new(&[]);
        let paths = vec![PathBuf::from("/a/mod.lgx")];
        let flakes = vec!["./sub#lgx".to_string()];
        let got = resolve_install_sources(
            tmp.path(),
            &paths,
            &flakes,
            &probe,
            &[],
            "lgx",
            "lgx-portable",
        )
        .expect("resolve");
        assert_eq!(
            got,
            vec![
                BasecampSource::Path("/a/mod.lgx".to_string()),
                BasecampSource::Flake("./sub#lgx".to_string()),
            ]
        );
    }

    #[test]
    fn resolve_install_sources_uses_root_flake_lgx() {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path();
        fs::write(root.join("flake.nix"), b"{}").unwrap();
        let root_ref = format!("path:{}", root.display());
        let probe = FakeProbe::new(&[(root_ref.as_str(), &["lgx", "default"])]);
        let got = resolve_install_sources(root, &[], &[], &probe, &[], "lgx", "lgx-portable")
            .expect("resolve");
        assert_eq!(got, vec![BasecampSource::Flake(format!("{root_ref}#lgx"))]);
    }

    #[test]
    fn resolve_install_sources_selects_portable_when_attr_is_lgx_portable() {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path();
        fs::write(root.join("flake.nix"), b"{}").unwrap();
        let root_ref = format!("path:{}", root.display());
        let probe = FakeProbe::new(&[(root_ref.as_str(), &["lgx", "lgx-portable"])]);
        let got = resolve_install_sources(root, &[], &[], &probe, &[], "lgx-portable", "lgx")
            .expect("resolve");
        assert_eq!(
            got,
            vec![BasecampSource::Flake(format!("{root_ref}#lgx-portable"))],
            "when requesting lgx-portable, the portable attr must win even if lgx is also present"
        );
    }

    #[test]
    fn resolve_install_sources_fails_when_requested_attr_absent() {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path();
        fs::write(root.join("flake.nix"), b"{}").unwrap();
        let root_ref = format!("path:{}", root.display());
        let probe = FakeProbe::new(&[(root_ref.as_str(), &["lgx"])]);
        let err = resolve_install_sources(root, &[], &[], &probe, &[], "lgx-portable", "lgx")
            .unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("lgx-portable") && msg.contains("lgx") && msg.contains("--flake"),
            "expected alt-only hint for requested lgx-portable, got: {msg}"
        );
    }

    #[test]
    fn resolve_install_sources_discovers_subflakes_when_root_missing_lgx() {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path();
        let sub_a = root.join("tictactoe");
        let sub_b = root.join("tictactoe-ui");
        let hidden = root.join(".not-a-sub");
        for d in [&sub_a, &sub_b, &hidden] {
            fs::create_dir_all(d).unwrap();
            fs::write(d.join("flake.nix"), b"{}").unwrap();
        }
        let refs = [
            (format!("path:{}", sub_a.display()), vec!["lgx"]),
            (format!("path:{}", sub_b.display()), vec!["lgx"]),
            (format!("path:{}", hidden.display()), vec!["lgx"]),
        ];
        let answers: Vec<(&str, &[&str])> = refs
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_slice()))
            .collect();
        let probe = FakeProbe::new(&answers);
        let got = resolve_install_sources(root, &[], &[], &probe, &[], "lgx", "lgx-portable")
            .expect("resolve");
        assert_eq!(
            got,
            vec![
                BasecampSource::Flake(format!("path:{}#lgx", sub_a.display())),
                BasecampSource::Flake(format!("path:{}#lgx", sub_b.display())),
            ],
            "hidden dotdirs must be skipped; results must be sorted"
        );
    }

    #[test]
    fn resolve_install_sources_portable_only_fails_with_hint() {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path();
        fs::write(root.join("flake.nix"), b"{}").unwrap();
        let root_ref = format!("path:{}", root.display());
        let probe = FakeProbe::new(&[(root_ref.as_str(), &["lgx-portable"])]);
        let err = resolve_install_sources(root, &[], &[], &probe, &[], "lgx", "lgx-portable")
            .unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("lgx-portable") && msg.contains("--flake"),
            "expected portable-only hint, got: {msg}"
        );
    }

    #[test]
    fn resolve_install_sources_skips_named_subdirs() {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path();
        for name in ["target", "cache"] {
            let d = root.join(name);
            fs::create_dir_all(&d).unwrap();
            fs::write(d.join("flake.nix"), b"{}").unwrap();
        }
        let real = root.join("real-mod");
        fs::create_dir_all(&real).unwrap();
        fs::write(real.join("flake.nix"), b"{}").unwrap();
        let answers_owned = vec![
            (
                format!("path:{}", root.join("target").display()),
                vec!["lgx"],
            ),
            (
                format!("path:{}", root.join("cache").display()),
                vec!["lgx"],
            ),
            (format!("path:{}", real.display()), vec!["lgx"]),
        ];
        let answers: Vec<(&str, &[&str])> = answers_owned
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_slice()))
            .collect();
        let probe = FakeProbe::new(&answers);
        let got = resolve_install_sources(
            root,
            &[],
            &[],
            &probe,
            &["target", "cache"],
            "lgx",
            "lgx-portable",
        )
        .expect("resolve");
        assert_eq!(
            got,
            vec![BasecampSource::Flake(format!(
                "path:{}#lgx",
                real.display()
            ))],
            "skip_subdirs must prune target/cache even if they contain flake.nix"
        );
    }

    #[test]
    fn resolve_install_sources_no_lgx_anywhere_fails_with_generic_hint() {
        let tmp = tempdir().expect("tempdir");
        let probe = FakeProbe::new(&[]);
        let err = resolve_install_sources(tmp.path(), &[], &[], &probe, &[], "lgx", "lgx-portable")
            .unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("--path") && msg.contains("--flake"),
            "expected generic hint, got: {msg}"
        );
    }

    #[test]
    fn lgpm_install_args_pins_global_flags_before_subcommand() {
        let args = lgpm_install_args(
            Path::new("/p/modules"),
            Path::new("/p/plugins"),
            Path::new("/p/mod.lgx"),
        );
        let rendered: Vec<String> = args
            .iter()
            .map(|o| o.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            rendered,
            vec![
                "--modules-dir",
                "/p/modules",
                "--ui-plugins-dir",
                "/p/plugins",
                "install",
                "--file",
                "/p/mod.lgx",
            ],
            "lgpm expects global --modules-dir / --ui-plugins-dir BEFORE the `install` subcommand"
        );
    }

    #[test]
    fn sibling_overrides_pair_path_flakes_under_a_shared_parent() {
        // Set up a repo with three sub-flakes sharing a parent and two
        // distractors (different-parent flake, path source, remote flake).
        // Target is the ui-cpp flake; it declares `tictactoe` as an input
        // pointing at the sibling. Only that sibling should produce an
        // override; the others must be filtered out.
        let tmp = tempdir().expect("tempdir");
        let repo = tmp.path().join("repo");
        let tictactoe = repo.join("tictactoe");
        let ui_cpp = repo.join("tictactoe-ui-cpp");
        fs::create_dir_all(&tictactoe).unwrap();
        fs::create_dir_all(&ui_cpp).unwrap();
        fs::write(
            ui_cpp.join("flake.nix"),
            r#"{ inputs.tictactoe.url = "path:../tictactoe"; outputs = {...}: {}; }"#,
        )
        .unwrap();

        let target = BasecampSource::Flake(format!("path:{}#lgx", ui_cpp.display()));
        let all = vec![
            BasecampSource::Flake(format!("path:{}#lgx", tictactoe.display())),
            BasecampSource::Flake(format!("path:{}#lgx", ui_cpp.display())),
            BasecampSource::Flake("path:/elsewhere/other#lgx".to_string()),
            BasecampSource::Path("/repo/prebuilt.lgx".to_string()),
            BasecampSource::Flake("github:foo/bar#lgx".to_string()),
        ];
        let got = sibling_overrides_for(&target, &all);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, "tictactoe");
        assert!(got[0].1.starts_with("path:/"));
        assert!(got[0].1.ends_with("tictactoe"));
    }

    #[test]
    fn sibling_overrides_use_declared_input_name_not_dirname() {
        // Target's flake declares `inputs.core.url = "path:../core-src"` —
        // input name `core`, sibling directory `core-src`. Override must key
        // by `core`, not `core-src`.
        let tmp = tempdir().expect("tempdir");
        let repo = tmp.path().join("repo");
        let core_src = repo.join("core-src");
        let ui = repo.join("ui");
        fs::create_dir_all(&core_src).unwrap();
        fs::create_dir_all(&ui).unwrap();
        fs::write(
            ui.join("flake.nix"),
            r#"{ inputs.core.url = "path:../core-src"; outputs = {...}: {}; }"#,
        )
        .unwrap();

        let target = BasecampSource::Flake(format!("path:{}#lgx", ui.display()));
        let all = vec![
            BasecampSource::Flake(format!("path:{}#lgx", core_src.display())),
            BasecampSource::Flake(format!("path:{}#lgx", ui.display())),
        ];
        let got = sibling_overrides_for(&target, &all);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, "core", "override must key by declared input name");
        assert!(got[0].1.ends_with("core-src"));
    }

    #[test]
    fn retain_declared_overrides_drops_unknown_names() {
        let declared: std::collections::HashSet<String> =
            ["tictactoe".to_string()].into_iter().collect();
        let kept = retain_declared_overrides(
            vec![
                ("tictactoe".to_string(), "path:/a".to_string()),
                ("tictactoe-ui-cpp".to_string(), "path:/b".to_string()),
                ("tictactoe-ui-qml".to_string(), "path:/c".to_string()),
            ],
            &declared,
        );
        assert_eq!(kept, vec![("tictactoe".to_string(), "path:/a".to_string())]);
    }

    #[test]
    fn retain_declared_overrides_empty_declared_drops_all() {
        let declared = std::collections::HashSet::new();
        let kept =
            retain_declared_overrides(vec![("x".to_string(), "path:/x".to_string())], &declared);
        assert!(kept.is_empty());
    }

    #[test]
    fn sibling_overrides_returns_empty_for_path_sources_and_remote_flakes() {
        let all = vec![
            BasecampSource::Flake("path:/repo/a#lgx".to_string()),
            BasecampSource::Flake("path:/repo/b#lgx".to_string()),
        ];
        assert!(
            sibling_overrides_for(&BasecampSource::Path("/anywhere.lgx".to_string()), &all)
                .is_empty(),
            "Path sources don't need overrides — they aren't built with nix"
        );
        assert!(
            sibling_overrides_for(&BasecampSource::Flake("github:x/y#lgx".to_string()), &all)
                .is_empty(),
            "remote flakes don't get sibling-override treatment (no shared local parent)"
        );
    }

    #[test]
    fn flake_out_link_name_avoids_slug_collisions() {
        // Two refs that would slugify to the same base name must not produce the same
        // out-link file, or one `nix build` will silently clobber the other.
        let a = flake_out_link_name("github:logos-co/x#lgx");
        let b = flake_out_link_name("github_logos_co_x_lgx");
        assert_ne!(a, b);
        assert!(a.ends_with("-result") && b.ends_with("-result"));
    }

    #[test]
    fn seed_profiles_creates_xdg_subdirs_for_each_name() {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path().join("profiles");

        let names = ["alice", "bob"];
        let seeded = seed_profiles(&root, &names).expect("seed");
        assert_eq!(seeded, vec!["alice".to_string(), "bob".to_string()]);

        for name in names {
            for xdg in ["xdg-config", "xdg-data", "xdg-cache"] {
                let dir = root.join(name).join(xdg).join(BASECAMP_XDG_APP_SUBPATH);
                assert!(dir.is_dir(), "expected XDG subdir at {}", dir.display());
            }
        }
    }

    #[test]
    fn launch_env_exports_xdg_under_profile_dir_and_profile_name() {
        let profile_dir = Path::new("/p/alice");
        let env = launch_env(profile_dir, "alice");
        assert_eq!(
            env.get("XDG_CONFIG_HOME").unwrap(),
            &OsString::from("/p/alice/xdg-config")
        );
        assert_eq!(
            env.get("XDG_DATA_HOME").unwrap(),
            &OsString::from("/p/alice/xdg-data")
        );
        assert_eq!(
            env.get("XDG_CACHE_HOME").unwrap(),
            &OsString::from("/p/alice/xdg-cache")
        );
        assert_eq!(env.get("LOGOS_PROFILE").unwrap(), &OsString::from("alice"));
    }

    #[test]
    fn scrub_removes_xdg_data_and_cache_but_keeps_config() {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path();
        let profile_dir = root.join(".scaffold/basecamp/profiles/alice");
        for xdg in ["xdg-data", "xdg-cache", "xdg-config"] {
            let d = profile_dir.join(xdg);
            fs::create_dir_all(&d).unwrap();
            fs::write(d.join("sentinel"), b"x").unwrap();
        }

        scrub_profile_data_and_cache(root, &profile_dir).expect("scrub");

        assert!(!profile_dir.join("xdg-data").exists());
        assert!(!profile_dir.join("xdg-cache").exists());
        assert!(
            profile_dir.join("xdg-config/sentinel").exists(),
            "xdg-config must be preserved (profile state the user may have edited)"
        );
    }

    #[test]
    fn scrub_refuses_paths_outside_profiles_root() {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path();
        fs::create_dir_all(root.join(".scaffold/basecamp/profiles")).unwrap();
        // Adversarial: profile_dir points somewhere else entirely.
        let outside = tmp.path().join("not-a-profile");
        fs::create_dir_all(outside.join("xdg-data")).unwrap();
        fs::write(outside.join("xdg-data/precious"), b"keep").unwrap();

        let err = scrub_profile_data_and_cache(root, &outside).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("refusing to scrub"), "got: {msg}");
        assert!(
            outside.join("xdg-data/precious").exists(),
            "scrub must not touch anything when the safety check fails"
        );
    }

    #[test]
    fn launch_pid_roundtrips_and_missing_returns_none() {
        let tmp = tempdir().expect("tempdir");
        let path = tmp.path().join("nested/launch.state");
        assert!(read_launch_pid(&path).is_none(), "missing file → None");

        write_launch_pid(&path, 42).expect("write");
        assert_eq!(read_launch_pid(&path), Some(42));

        fs::write(&path, "garbage\n").unwrap();
        assert!(read_launch_pid(&path).is_none(), "malformed file → None");
    }

    #[test]
    fn write_launch_pid_leaves_no_tmp_file_on_success() {
        let tmp = tempdir().expect("tempdir");
        let path = tmp.path().join("launch.state");
        write_launch_pid(&path, 99).expect("write");
        // The atomic-write helper uses `<path>.state.tmp` as its staging file.
        assert!(
            !path.with_extension("state.tmp").exists(),
            "tmp file must be renamed, not left behind"
        );
    }

    #[test]
    fn scrub_succeeds_on_first_run_when_safe_root_doesnt_exist_yet() {
        // Regression: canonicalize() requires every component to exist. Before the
        // fix, a first-run scrub against a fresh project errored out because
        // `.scaffold/basecamp/profiles` hadn't been created yet.
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path();
        let profile_dir = root.join(".scaffold/basecamp/profiles/alice");
        fs::create_dir_all(profile_dir.join("xdg-data")).unwrap();
        fs::create_dir_all(profile_dir.join("xdg-cache")).unwrap();
        scrub_profile_data_and_cache(root, &profile_dir).expect("scrub");
        assert!(!profile_dir.join("xdg-data").exists());
    }

    #[test]
    fn canonicalize_under_errors_when_parent_is_missing() {
        // Silent fallback would be a safety-check bypass — verify we fail loudly.
        let tmp = tempdir().expect("tempdir");
        let err = canonicalize_under(&tmp.path().join("no/such/parent/alice")).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("canonicalize"), "got: {msg}");
    }

    #[test]
    fn basecamp_comm_name_truncates_to_15_bytes_like_proc_comm() {
        assert_eq!(
            basecamp_comm_name("/nix/store/xyz/bin/basecamp"),
            "basecamp"
        );
        assert_eq!(
            basecamp_comm_name("/x/extremely-long-binary-name"),
            "extremely-long-"
        );
    }

    #[test]
    fn pid_comm_matches_returns_false_for_reserved_pid_0() {
        // PID 0 is reserved and `ps -p 0` reliably fails, standing in for any PID
        // where we can't recover a comm — the helper must fail closed so the kill
        // path is skipped rather than firing at a wrong target.
        assert!(!pid_comm_matches(0, "basecamp"));
    }

    #[test]
    fn seed_profiles_is_idempotent() {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path().join("profiles");
        seed_profiles(&root, &["alice"]).expect("first");
        // Drop a sentinel file inside the xdg-data dir; a second seed must not delete it.
        let sentinel = root
            .join("alice/xdg-data")
            .join(BASECAMP_XDG_APP_SUBPATH)
            .join("keep-me.txt");
        fs::write(&sentinel, b"hi").expect("write sentinel");
        seed_profiles(&root, &["alice"]).expect("second");
        assert!(
            sentinel.exists(),
            "second seed must not scrub existing contents"
        );
    }

    // ---- build-portable helpers ----

    #[test]
    fn build_portable_nix_invocation_path_ref_cds_into_flake_dir() {
        let inv = build_portable_nix_invocation("path:/abs/to/foo#lgx-portable", &[]);
        assert_eq!(inv.cwd_override.as_deref(), Some(Path::new("/abs/to/foo")));
        assert_eq!(
            inv.args,
            vec!["build", ".#lgx-portable", "--print-out-paths"]
        );
    }

    #[test]
    fn build_portable_nix_invocation_remote_ref_stays_in_project_root() {
        let inv = build_portable_nix_invocation("github:foo/bar#lgx-portable", &[]);
        assert!(
            inv.cwd_override.is_none(),
            "remote refs must not override cwd"
        );
        assert_eq!(
            inv.args,
            vec!["build", "github:foo/bar#lgx-portable", "--print-out-paths"]
        );
    }

    #[test]
    fn build_portable_nix_invocation_does_not_use_out_link() {
        // Spec: `nix build` without `-o`, so the default `./result-<attr>` symlink
        // lands next to the flake. No `--out-link`, no `--no-link`.
        let inv = build_portable_nix_invocation("path:/abs/a#lgx-portable", &[]);
        for forbidden in ["-o", "--out-link", "--no-link"] {
            assert!(
                !inv.args.iter().any(|a| a == forbidden),
                "argv must not contain `{forbidden}`: {:?}",
                inv.args
            );
        }
    }

    #[test]
    fn build_portable_nix_invocation_inserts_overrides_before_print_out_paths() {
        let inv = build_portable_nix_invocation(
            "path:/abs/ui#lgx-portable",
            &[("core".to_string(), "path:/abs/core".to_string())],
        );
        assert_eq!(
            inv.args,
            vec![
                "build",
                ".#lgx-portable",
                "--override-input",
                "core",
                "path:/abs/core",
                "--print-out-paths",
            ]
        );
    }

    #[test]
    fn build_portable_resolve_path_returns_canonical_absolute_lgx() {
        let tmp = tempdir().expect("tempdir");
        let p = tmp.path().join("module.lgx");
        fs::write(&p, b"fake lgx").unwrap();
        let got = build_portable_resolve_path(&p).expect("ok");
        assert!(got.is_absolute(), "got: {}", got.display());
        assert_eq!(got.canonicalize().unwrap(), p.canonicalize().unwrap());
    }

    #[test]
    fn build_portable_resolve_path_rejects_non_lgx_extension() {
        let tmp = tempdir().expect("tempdir");
        let p = tmp.path().join("module.txt");
        fs::write(&p, b"not lgx").unwrap();
        let err = build_portable_resolve_path(&p).unwrap_err();
        assert!(
            format!("{err}").contains("not a .lgx file"),
            "expected extension-rejection hint, got: {err}"
        );
    }

    // ---- normalize_flake_ref (I2 fix) ----

    #[test]
    fn normalize_flake_ref_rewrites_relative_path_to_path_scheme() {
        let tmp = tempdir().expect("tempdir");
        let sub = tmp.path().join("sub");
        fs::create_dir_all(&sub).unwrap();
        let got = normalize_flake_ref(tmp.path(), "./sub#lgx-portable");
        assert!(got.starts_with("path:"), "got: {got}");
        assert!(got.ends_with("#lgx-portable"), "got: {got}");
        let canon = sub.canonicalize().unwrap();
        assert!(
            got.contains(canon.to_str().unwrap()),
            "expected canonical sub path in {got}"
        );
    }

    #[test]
    fn normalize_flake_ref_rewrites_absolute_path_to_path_scheme() {
        let tmp = tempdir().expect("tempdir");
        let sub = tmp.path().join("abs-sub");
        fs::create_dir_all(&sub).unwrap();
        let abs_spec = format!("{}#lgx", sub.display());
        let got = normalize_flake_ref(Path::new("/"), &abs_spec);
        assert!(got.starts_with("path:"), "got: {got}");
        assert!(got.ends_with("#lgx"), "got: {got}");
    }

    #[test]
    fn normalize_flake_ref_passes_through_scheme_refs() {
        let tmp = tempdir().expect("tempdir");
        for input in [
            "path:/abs/p#lgx",
            "github:foo/bar#lgx-portable",
            "git+https://example/r#lgx",
            "https://example/archive.tar.gz#lgx",
            "gitlab:foo/bar#lgx",
        ] {
            assert_eq!(
                normalize_flake_ref(tmp.path(), input),
                input,
                "scheme ref must pass through unchanged"
            );
        }
    }

    #[test]
    fn normalize_flake_ref_preserves_missing_fragment() {
        let tmp = tempdir().expect("tempdir");
        let sub = tmp.path().join("nofrag");
        fs::create_dir_all(&sub).unwrap();
        let got = normalize_flake_ref(tmp.path(), "./nofrag");
        assert!(
            got.starts_with("path:") && !got.contains('#'),
            "no fragment means no `#` in output: {got}"
        );
    }

    #[test]
    fn normalize_flake_ref_absolutizes_relative_path_scheme_refs() {
        // Relative `path:./sub#lgx` persisted to scaffold.toml must be
        // re-anchored to project_root so downstream nix invocations see a
        // single absolute `path:<abs>#lgx` shape.
        let tmp = tempdir().expect("tempdir");
        let sub = tmp.path().join("sub");
        fs::create_dir_all(&sub).unwrap();

        let got = normalize_flake_ref(tmp.path(), "path:./sub#lgx");
        assert!(got.starts_with("path:/"), "got: {got}");
        assert!(got.ends_with("#lgx"), "got: {got}");
        let canon = sub.canonicalize().unwrap();
        assert!(
            got.contains(canon.to_str().unwrap()),
            "expected canonical sub path in {got}"
        );

        let got_root = normalize_flake_ref(tmp.path(), "path:.#lgx");
        let canon_root = tmp.path().canonicalize().unwrap();
        assert!(
            got_root.contains(canon_root.to_str().unwrap()),
            "expected canonical root in {got_root}"
        );
    }

    #[test]
    fn relativize_flake_ref_rewrites_in_project_absolute_refs() {
        let tmp = tempdir().expect("tempdir");
        let sub = tmp.path().join("sub");
        fs::create_dir_all(&sub).unwrap();
        let canon_sub = sub.canonicalize().unwrap();
        let abs_ref = format!("path:{}#lgx", canon_sub.display());

        let got = relativize_flake_ref(tmp.path(), &abs_ref);
        assert_eq!(got, "path:./sub#lgx", "got: {got}");
    }

    #[test]
    fn relativize_flake_ref_rewrites_project_root_as_dot() {
        let tmp = tempdir().expect("tempdir");
        let canon_root = tmp.path().canonicalize().unwrap();
        let got = relativize_flake_ref(tmp.path(), &format!("path:{}#lgx", canon_root.display()));
        assert_eq!(got, "path:.#lgx", "got: {got}");
    }

    #[test]
    fn relativize_flake_ref_preserves_out_of_project_refs() {
        let tmp_proj = tempdir().expect("proj tempdir");
        let tmp_out = tempdir().expect("out tempdir");
        let canon_out = tmp_out.path().canonicalize().unwrap();
        let abs_ref = format!("path:{}#lgx", canon_out.display());

        let got = relativize_flake_ref(tmp_proj.path(), &abs_ref);
        assert_eq!(got, abs_ref, "out-of-project ref must stay absolute");
    }

    #[test]
    fn relativize_flake_ref_passes_through_non_path_refs() {
        let tmp = tempdir().expect("tempdir");
        for input in [
            "github:foo/bar#lgx",
            "git+https://example/r#lgx",
            "path:./already-relative#lgx",
        ] {
            assert_eq!(
                relativize_flake_ref(tmp.path(), input),
                input,
                "non-abs-path ref must pass through unchanged"
            );
        }
    }

    #[test]
    fn module_entry_to_source_normalizes_relative_flake_refs() {
        let tmp = tempdir().expect("tempdir");
        let sub = tmp.path().join("sub");
        fs::create_dir_all(&sub).unwrap();
        let entry = ModuleEntry {
            flake: "path:./sub#lgx".to_string(),
            role: ModuleRole::Project,
        };
        match module_entry_to_source(tmp.path(), &entry) {
            BasecampSource::Flake(f) => {
                assert!(f.starts_with("path:/"), "got: {f}");
                let canon = sub.canonicalize().unwrap();
                assert!(f.contains(canon.to_str().unwrap()), "got: {f}");
            }
            other => panic!("expected Flake source, got {other:?}"),
        }
    }

    // ---- resolve_sibling_overrides (I1 fix) — exercised via the command paths;
    //      a focused test pins the helper's Path-source short-circuit ----

    #[test]
    fn resolve_sibling_overrides_returns_empty_for_isolated_flake() {
        let only = BasecampSource::Flake("path:/abs/alone#lgx".to_string());
        let got =
            resolve_sibling_overrides(&only, std::slice::from_ref(&only), "path:/abs/alone#lgx");
        assert!(got.is_empty(), "no siblings → no overrides");
    }

    #[test]
    fn probe_sibling_overrides_keys_by_input_name_not_dirname() {
        // Target's flake.nix declares `tictactoe_solo_ai.url = "path:../logos-tictactoe-solo-ai"`.
        // The sibling dir on disk is `logos-tictactoe-solo-ai`. Override must
        // key by the input name `tictactoe_solo_ai`, not the dirname — nix
        // would warn + ignore the latter.
        let tmp = tempdir().expect("tempdir");
        let target = tmp.path().join("ui");
        let sibling = tmp.path().join("logos-tictactoe-solo-ai");
        fs::create_dir_all(&target).unwrap();
        fs::create_dir_all(&sibling).unwrap();
        fs::write(
            target.join("flake.nix"),
            r#"{
  inputs.tictactoe_solo_ai.url = "path:../logos-tictactoe-solo-ai";
  outputs = { ... }: {};
}"#,
        )
        .unwrap();

        let siblings: Vec<&Path> = vec![&sibling];
        let overrides = probe_sibling_overrides(&target, &siblings);
        assert_eq!(overrides.len(), 1);
        assert_eq!(overrides[0].0, "tictactoe_solo_ai");
        assert!(overrides[0].1.starts_with("path:/"));
        assert!(overrides[0].1.ends_with("logos-tictactoe-solo-ai"));
    }

    #[test]
    fn probe_sibling_overrides_handles_bare_inputs_block_form() {
        // `inputs = { foo.url = "..."; }` — bare `foo.url` without `inputs.` prefix.
        let tmp = tempdir().expect("tempdir");
        let target = tmp.path().join("ui");
        let sibling = tmp.path().join("core");
        fs::create_dir_all(&target).unwrap();
        fs::create_dir_all(&sibling).unwrap();
        fs::write(
            target.join("flake.nix"),
            r#"{
  inputs = {
    core.url = "path:../core";
    nixpkgs.url = "github:NixOS/nixpkgs";
  };
  outputs = { ... }: {};
}"#,
        )
        .unwrap();

        let siblings: Vec<&Path> = vec![&sibling];
        let overrides = probe_sibling_overrides(&target, &siblings);
        assert_eq!(overrides.len(), 1);
        assert_eq!(overrides[0].0, "core");
    }

    #[test]
    fn probe_sibling_overrides_skips_non_sibling_path_refs() {
        // Input references `path:../other` but `other` isn't one of the
        // siblings we know about on disk → no override emitted. Safer than
        // emitting a bogus path and triggering a second nix failure.
        let tmp = tempdir().expect("tempdir");
        let target = tmp.path().join("ui");
        fs::create_dir_all(&target).unwrap();
        fs::write(
            target.join("flake.nix"),
            r#"{ inputs.other.url = "path:../other"; outputs = {}: {}; }"#,
        )
        .unwrap();

        assert!(probe_sibling_overrides(&target, &[]).is_empty());
    }

    #[test]
    fn probe_sibling_overrides_skips_non_path_inputs() {
        let tmp = tempdir().expect("tempdir");
        let target = tmp.path().join("ui");
        fs::create_dir_all(&target).unwrap();
        fs::write(
            target.join("flake.nix"),
            r#"{
  inputs.nixpkgs.url = "github:NixOS/nixpkgs";
  inputs.lib.url = "path:./lib";
  outputs = {...}: {};
}"#,
        )
        .unwrap();
        assert!(probe_sibling_overrides(&target, &[]).is_empty());
    }

    #[test]
    fn probe_sibling_overrides_returns_empty_when_no_flake_nix() {
        let tmp = tempdir().expect("tempdir");
        assert!(probe_sibling_overrides(tmp.path(), &[]).is_empty());
    }

    #[test]
    fn parse_path_dotdot_inputs_extracts_input_name_and_sibling_stem() {
        let got = parse_path_dotdot_inputs(
            r#"{
  inputs.foo.url = "path:../foo";
  inputs = {
    bar_baz.url = "path:../shared-bar-baz";
  };
}"#,
        );
        assert_eq!(got.len(), 2);
        assert!(got.contains(&("foo".to_string(), "foo".to_string())));
        assert!(got.contains(&("bar_baz".to_string(), "shared-bar-baz".to_string())));
    }

    // ---- `basecamp modules` helpers (manifest walking + dep resolution) ----

    fn entry_for(flake: &str) -> ModuleEntry {
        ModuleEntry {
            flake: flake.to_string(),
            role: ModuleRole::Project,
        }
    }

    #[test]
    fn topo_order_empty_returns_empty() {
        let modules: std::collections::BTreeMap<String, ModuleEntry> =
            std::collections::BTreeMap::new();
        assert!(topo_order_project_modules(Path::new("/"), &modules).is_empty());
    }

    #[test]
    fn topo_order_single_module_returns_singleton() {
        let tmp = tempdir().expect("tempdir");
        let d = tmp.path().join("only");
        seed_module_metadata(&d, "only", &[]);
        let mut modules = std::collections::BTreeMap::new();
        modules.insert(
            "only".to_string(),
            entry_for(&format!("path:{}#lgx", d.display())),
        );
        assert_eq!(
            topo_order_project_modules(tmp.path(), &modules),
            vec!["only".to_string()]
        );
    }

    #[test]
    fn topo_order_emits_dep_before_dependent() {
        // A declares dep on B; build-portable load order must be [B, A] so
        // basecamp can resolve B's symbols before loading A.
        let tmp = tempdir().expect("tempdir");
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        seed_module_metadata(&a, "a", &["b"]);
        seed_module_metadata(&b, "b", &[]);
        let mut modules = std::collections::BTreeMap::new();
        modules.insert(
            "a".to_string(),
            entry_for(&format!("path:{}#lgx", a.display())),
        );
        modules.insert(
            "b".to_string(),
            entry_for(&format!("path:{}#lgx", b.display())),
        );
        assert_eq!(
            topo_order_project_modules(tmp.path(), &modules),
            vec!["b".to_string(), "a".to_string()]
        );
    }

    #[test]
    fn topo_order_handles_three_level_chain() {
        let tmp = tempdir().expect("tempdir");
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        let c = tmp.path().join("c");
        seed_module_metadata(&a, "a", &["b"]);
        seed_module_metadata(&b, "b", &["c"]);
        seed_module_metadata(&c, "c", &[]);
        let mut modules = std::collections::BTreeMap::new();
        for (name, dir) in [("a", &a), ("b", &b), ("c", &c)] {
            modules.insert(
                name.to_string(),
                entry_for(&format!("path:{}#lgx", dir.display())),
            );
        }
        assert_eq!(
            topo_order_project_modules(tmp.path(), &modules),
            vec!["c".to_string(), "b".to_string(), "a".to_string()]
        );
    }

    #[test]
    fn topo_order_sorts_peers_alphabetically_for_determinism() {
        // A depends on both b and c; b and c have no deps between each other.
        // Emit peers alphabetically so build-portable output is reproducible.
        let tmp = tempdir().expect("tempdir");
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        let c = tmp.path().join("c");
        seed_module_metadata(&a, "a", &["b", "c"]);
        seed_module_metadata(&b, "b", &[]);
        seed_module_metadata(&c, "c", &[]);
        let mut modules = std::collections::BTreeMap::new();
        for (name, dir) in [("a", &a), ("b", &b), ("c", &c)] {
            modules.insert(
                name.to_string(),
                entry_for(&format!("path:{}#lgx", dir.display())),
            );
        }
        assert_eq!(
            topo_order_project_modules(tmp.path(), &modules),
            vec!["b".to_string(), "c".to_string(), "a".to_string()]
        );
    }

    #[test]
    fn topo_order_ignores_non_project_deps() {
        // A declares `delivery_module` as a dep but delivery_module isn't
        // in the project-module set (it's a role=Dependency entry). The
        // dep doesn't participate in project ordering.
        let tmp = tempdir().expect("tempdir");
        let a = tmp.path().join("a");
        seed_module_metadata(&a, "a", &["delivery_module"]);
        let mut modules = std::collections::BTreeMap::new();
        modules.insert(
            "a".to_string(),
            entry_for(&format!("path:{}#lgx", a.display())),
        );
        assert_eq!(
            topo_order_project_modules(tmp.path(), &modules),
            vec!["a".to_string()]
        );
    }

    #[test]
    fn topo_order_breaks_cycle_with_stable_fallback() {
        // A <-> B cycle. Shouldn't deadlock; fall back to emitting the
        // cycle members in alphabetical order so the output is at least
        // deterministic even if it won't load correctly.
        let tmp = tempdir().expect("tempdir");
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        seed_module_metadata(&a, "a", &["b"]);
        seed_module_metadata(&b, "b", &["a"]);
        let mut modules = std::collections::BTreeMap::new();
        modules.insert(
            "a".to_string(),
            entry_for(&format!("path:{}#lgx", a.display())),
        );
        modules.insert(
            "b".to_string(),
            entry_for(&format!("path:{}#lgx", b.display())),
        );
        let got = topo_order_project_modules(tmp.path(), &modules);
        assert_eq!(got.len(), 2);
        assert!(got.contains(&"a".to_string()) && got.contains(&"b".to_string()));
    }

    fn seed_module_metadata(dir: &Path, name: &str, deps: &[&str]) {
        let metadata = serde_json::json!({
            "name": name,
            "version": "1.0.0",
            "type": "core",
            "main": "plugin",
            "dependencies": deps,
        });
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join("metadata.json"), metadata.to_string()).unwrap();
    }

    fn write_flake_lock(dir: &Path, text: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join("flake.lock"), text).unwrap();
    }

    #[test]
    fn resolve_dep_from_project_flake_lock_returns_github_ref_for_plain_input() {
        let tmp = tempdir().expect("tempdir");
        write_flake_lock(
            tmp.path(),
            r#"{
  "nodes": {
    "root": { "inputs": { "delivery_module": "delivery_module" } },
    "delivery_module": {
      "locked": {
        "type": "github",
        "owner": "logos-co",
        "repo": "logos-delivery-module",
        "rev": "abc123"
      }
    }
  }
}"#,
        );
        let got = resolve_dep_from_project_flake_lock(tmp.path(), "delivery_module");
        assert_eq!(
            got.as_deref(),
            Some("github:logos-co/logos-delivery-module/abc123#lgx")
        );
    }

    #[test]
    fn resolve_dep_from_project_flake_lock_returns_none_for_follows_array_input() {
        // Nix emits `"inputs": { "shared": ["foo", "bar"] }` when a flake
        // declares `inputs.shared.follows = "foo/bar"`. That value is an
        // array, not a string node-id — resolver must return None.
        let tmp = tempdir().expect("tempdir");
        write_flake_lock(
            tmp.path(),
            r#"{
  "nodes": {
    "root": { "inputs": { "shared": ["foo", "bar"] } }
  }
}"#,
        );
        assert_eq!(
            resolve_dep_from_project_flake_lock(tmp.path(), "shared"),
            None
        );
    }

    #[test]
    fn resolve_dep_from_project_flake_lock_returns_none_for_non_github_locked_type() {
        let tmp = tempdir().expect("tempdir");
        write_flake_lock(
            tmp.path(),
            r#"{
  "nodes": {
    "root": { "inputs": { "local_lib": "local_lib" } },
    "local_lib": {
      "locked": { "type": "path", "path": "/abs/local_lib" }
    }
  }
}"#,
        );
        assert_eq!(
            resolve_dep_from_project_flake_lock(tmp.path(), "local_lib"),
            None
        );
    }

    #[test]
    fn resolve_dep_from_project_flake_lock_returns_none_when_input_missing() {
        let tmp = tempdir().expect("tempdir");
        write_flake_lock(
            tmp.path(),
            r#"{
  "nodes": {
    "root": { "inputs": { "other": "other" } },
    "other": {
      "locked": { "type": "github", "owner": "x", "repo": "y", "rev": "z" }
    }
  }
}"#,
        );
        assert_eq!(
            resolve_dep_from_project_flake_lock(tmp.path(), "delivery_module"),
            None
        );
    }

    #[test]
    fn resolve_dep_from_project_flake_lock_returns_none_when_file_missing() {
        let tmp = tempdir().expect("tempdir");
        // No flake.lock written at all.
        assert_eq!(
            resolve_dep_from_project_flake_lock(tmp.path(), "delivery_module"),
            None
        );
    }

    #[test]
    fn resolve_dep_from_project_flake_lock_returns_none_for_malformed_json() {
        let tmp = tempdir().expect("tempdir");
        write_flake_lock(tmp.path(), "{ this is not valid json");
        assert_eq!(
            resolve_dep_from_project_flake_lock(tmp.path(), "delivery_module"),
            None
        );
    }

    #[test]
    fn resolve_dep_from_project_flake_lock_follows_suffixed_node_id() {
        // When two inputs share a name, nix renames the second to `<name>_2`
        // and the root inputs entry points at that suffixed node-id. The
        // declared input name (the root inputs key) is still the plain name.
        let tmp = tempdir().expect("tempdir");
        write_flake_lock(
            tmp.path(),
            r#"{
  "nodes": {
    "root": { "inputs": { "delivery_module": "delivery_module_2" } },
    "delivery_module_2": {
      "locked": {
        "type": "github",
        "owner": "logos-co",
        "repo": "logos-delivery-module",
        "rev": "def456"
      }
    }
  }
}"#,
        );
        assert_eq!(
            resolve_dep_from_project_flake_lock(tmp.path(), "delivery_module").as_deref(),
            Some("github:logos-co/logos-delivery-module/def456#lgx")
        );
    }

    #[test]
    fn read_source_metadata_dependencies_parses_array() {
        let tmp = tempdir().expect("tempdir");
        seed_module_metadata(tmp.path(), "mymod", &["delivery_module", "storage_module"]);
        let src = BasecampSource::Flake(format!("path:{}#lgx", tmp.path().display()));
        let deps = read_source_metadata_dependencies(&src);
        assert_eq!(
            deps,
            vec!["delivery_module".to_string(), "storage_module".to_string()]
        );
    }

    #[test]
    fn read_source_metadata_dependencies_returns_empty_for_missing_metadata() {
        let tmp = tempdir().expect("tempdir");
        let src = BasecampSource::Flake(format!("path:{}#lgx", tmp.path().display()));
        assert!(read_source_metadata_dependencies(&src).is_empty());
    }

    #[test]
    fn read_source_metadata_dependencies_returns_empty_for_path_sources() {
        let src = BasecampSource::Path("/abs/prebuilt.lgx".to_string());
        assert!(read_source_metadata_dependencies(&src).is_empty());
    }

    #[test]
    fn read_source_metadata_dependencies_returns_empty_for_remote_flakes() {
        let src = BasecampSource::Flake("github:foo/bar#lgx".to_string());
        assert!(read_source_metadata_dependencies(&src).is_empty());
    }

    #[test]
    fn resolve_manifest_dependencies_falls_back_to_scaffold_default() {
        let tmp = tempdir().expect("tempdir");
        seed_module_metadata(tmp.path(), "mymod", &["delivery_module"]);
        let project = BasecampSource::Flake(format!("path:{}#lgx", tmp.path().display()));

        let captured = std::collections::BTreeMap::new();
        let new_entries = resolve_manifest_dependencies(&[project], &captured).expect("ok");
        let entry = new_entries
            .get("delivery_module")
            .expect("delivery resolved");
        assert_eq!(entry.role, ModuleRole::Dependency);
        assert!(
            entry.flake.contains("logos-delivery-module"),
            "expected scaffold default delivery ref, got {}",
            entry.flake,
        );
    }

    #[test]
    fn resolve_manifest_dependencies_silently_skips_preinstalled_modules() {
        let tmp = tempdir().expect("tempdir");
        seed_module_metadata(
            tmp.path(),
            "mymod",
            &["capability_module", "package_manager"],
        );
        let project = BasecampSource::Flake(format!("path:{}#lgx", tmp.path().display()));
        let new_entries =
            resolve_manifest_dependencies(&[project], &std::collections::BTreeMap::new())
                .expect("ok");
        assert!(
            new_entries.is_empty(),
            "preinstalled modules must not be captured"
        );
    }

    #[test]
    fn resolve_manifest_dependencies_skips_names_already_in_captured_table() {
        // V-4 fix: a dep name that already has a `[basecamp.modules]` entry
        // (regardless of role) is silently skipped. vpavlin's repro: yolo
        // declares `storage_module` as a dep; the user captured
        // logos-storage-module as a project source via explicit --flake,
        // which resolves to module_name `storage_module`. The resolver must
        // see that and skip it.
        let tmp = tempdir().expect("tempdir");
        seed_module_metadata(tmp.path(), "yolo", &["storage_module"]);
        let project = BasecampSource::Flake(format!("path:{}#lgx", tmp.path().display()));

        let mut captured = std::collections::BTreeMap::new();
        captured.insert(
            "storage_module".to_string(),
            ModuleEntry {
                flake: "github:logos-co/logos-storage-module/abc123#lgx".to_string(),
                role: ModuleRole::Project,
            },
        );

        let new_entries = resolve_manifest_dependencies(&[project], &captured).expect("ok");
        assert!(
            new_entries.is_empty(),
            "dep already in captured table must not produce a new entry"
        );
    }

    #[test]
    fn resolve_manifest_dependencies_fails_fast_on_unresolved_dep() {
        let tmp = tempdir().expect("tempdir");
        seed_module_metadata(tmp.path(), "mymod", &["no_such_module"]);
        let project = BasecampSource::Flake(format!("path:{}#lgx", tmp.path().display()));

        let captured = std::collections::BTreeMap::new();
        let err = resolve_manifest_dependencies(&[project], &captured).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("no_such_module") && msg.contains("unresolved"),
            "expected fail-fast error naming the dep, got: {msg}"
        );
        assert!(
            msg.contains("basecamp modules --flake") || msg.contains("[basecamp.modules."),
            "expected both user-side fixes in error, got: {msg}"
        );
        assert!(
            msg.contains("basecamp docs"),
            "compatibility-related errors must point at `basecamp docs`, got: {msg}"
        );
    }

    #[test]
    fn resolve_manifest_dependencies_tolerates_mutual_cycle() {
        // Two project sources that each declare the other as a metadata.json
        // dependency. The resolver must not recurse indefinitely: each module
        // is already in `captured` as a project source, so the "already keyed"
        // check silently skips the cross-references. Produces no new deps,
        // completes in bounded time, no stack overflow.
        let tmp_a = tempdir().expect("tempdir a");
        seed_module_metadata(tmp_a.path(), "a_mod", &["b_mod"]);
        let src_a = BasecampSource::Flake(format!("path:{}#lgx", tmp_a.path().display()));

        let tmp_b = tempdir().expect("tempdir b");
        seed_module_metadata(tmp_b.path(), "b_mod", &["a_mod"]);
        let src_b = BasecampSource::Flake(format!("path:{}#lgx", tmp_b.path().display()));

        // Both modules have already been captured as project sources (the
        // normal capture flow keys them into `captured` before dep walking).
        let mut captured = std::collections::BTreeMap::new();
        captured.insert(
            "a_mod".to_string(),
            ModuleEntry {
                flake: format!("path:{}#lgx", tmp_a.path().display()),
                role: ModuleRole::Project,
            },
        );
        captured.insert(
            "b_mod".to_string(),
            ModuleEntry {
                flake: format!("path:{}#lgx", tmp_b.path().display()),
                role: ModuleRole::Project,
            },
        );

        let new_entries =
            resolve_manifest_dependencies(&[src_a, src_b], &captured).expect("no error on cycle");
        assert!(
            new_entries.is_empty(),
            "mutual cycle between already-captured projects must not produce dep entries, got {:?}",
            new_entries
        );
    }

    #[test]
    fn github_flake_ref_rev_extracts_middle_segment() {
        assert_eq!(
            github_flake_ref_rev("github:logos-co/logos-delivery-module/1fde1566#lgx"),
            Some("1fde1566")
        );
        assert_eq!(
            github_flake_ref_rev("github:foo/bar/1.0.0#lgx"),
            Some("1.0.0")
        );
    }

    #[test]
    fn github_flake_ref_rev_returns_none_for_non_github() {
        assert_eq!(github_flake_ref_rev("path:/abs#lgx"), None);
        assert_eq!(github_flake_ref_rev("github:foo/bar#lgx"), None);
    }

    fn write_metadata(dir: &Path, name: &str) {
        fs::write(
            dir.join("metadata.json"),
            format!("{{\"name\": \"{name}\", \"dependencies\": []}}"),
        )
        .expect("write metadata.json");
    }

    fn seed_basecamp_project(root: &Path) -> Project {
        let scaffold_toml = r#"[scaffold]
version = "0.2.0"
cache_root = "cache"

[repos.lez]
source = "s"
pin = "q"

[repos.spel]
source = "s"
pin = "q"

[repos.basecamp]
source = "https://example/basecamp"
pin = "deadbeef"
build = "nix-flake"
attr = "app"
"#;
        fs::write(root.join("scaffold.toml"), scaffold_toml).expect("write scaffold.toml");
        load_project_at(root)
    }

    fn load_project_at(root: &Path) -> Project {
        let text = fs::read_to_string(root.join("scaffold.toml")).expect("read");
        let cfg = crate::config::parse_config(&text).expect("parse");
        Project {
            root: root.to_path_buf(),
            config: cfg,
        }
    }

    #[test]
    fn cmd_basecamp_modules_writes_project_entries_into_scaffold_toml() {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path();
        // Source dir with metadata.json so derive_module_name reads it exactly.
        let src_dir = root.join("tictactoe");
        fs::create_dir_all(&src_dir).unwrap();
        write_metadata(&src_dir, "tictactoe_core");

        let project = seed_basecamp_project(root);
        let probe = FakeProbe::new(&[]);
        cmd_basecamp_modules(
            project,
            Vec::new(),
            vec![format!("path:{}#lgx", src_dir.display())],
            false,
            &probe,
        )
        .expect("modules ok");

        let text = fs::read_to_string(root.join("scaffold.toml")).unwrap();
        assert!(
            text.contains("[modules.tictactoe_core]"),
            "expected new module entry, got:\n{text}"
        );
        assert!(text.contains("role = \"project\""));
        // Auto-captured in-project flake refs are persisted in relative form
        // so the committed scaffold.toml is portable across clones/CI.
        assert!(
            text.contains("flake = \"path:./tictactoe#lgx\""),
            "expected relative flake ref, got:\n{text}"
        );
    }

    #[test]
    fn cmd_basecamp_modules_is_idempotent_on_rerun() {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path();
        let src_dir = root.join("mod");
        fs::create_dir_all(&src_dir).unwrap();
        write_metadata(&src_dir, "mod_name");

        let flake = format!("path:{}#lgx", src_dir.display());
        let project = seed_basecamp_project(root);
        let probe = FakeProbe::new(&[]);
        cmd_basecamp_modules(project, Vec::new(), vec![flake.clone()], false, &probe)
            .expect("first run");

        let first = fs::read_to_string(root.join("scaffold.toml")).unwrap();
        let project2 = load_project_at(root);
        cmd_basecamp_modules(project2, Vec::new(), vec![flake], false, &probe).expect("second run");
        let second = fs::read_to_string(root.join("scaffold.toml")).unwrap();

        assert_eq!(first, second, "re-running modules should be a no-op");
    }

    #[test]
    fn cmd_basecamp_modules_preserves_existing_entry_for_same_module_name() {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path();
        let src_dir = root.join("mod");
        fs::create_dir_all(&src_dir).unwrap();
        write_metadata(&src_dir, "shared_name");

        // Pre-seed scaffold.toml with an entry under `shared_name` that points
        // to a _different_ flake. Running modules against the local source
        // must NOT overwrite that entry (user intent wins).
        let scaffold_toml = r#"[scaffold]
version = "0.2.0"
cache_root = "cache"

[repos.lez]
source = "s"
pin = "q"

[repos.spel]
source = "s"
pin = "q"

[repos.basecamp]
source = "https://example/basecamp"
pin = "deadbeef"
build = "nix-flake"
attr = "app"

[modules.shared_name]
flake = "github:custom/fork/abc123#lgx"
role = "dependency"
"#;
        fs::write(root.join("scaffold.toml"), scaffold_toml).unwrap();

        let project = load_project_at(root);
        let probe = FakeProbe::new(&[]);
        cmd_basecamp_modules(
            project,
            Vec::new(),
            vec![format!("path:{}#lgx", src_dir.display())],
            false,
            &probe,
        )
        .expect("modules ok");

        let text = fs::read_to_string(root.join("scaffold.toml")).unwrap();
        assert!(
            text.contains("flake = \"github:custom/fork/abc123#lgx\""),
            "pre-existing entry must be preserved, got:\n{text}"
        );
        assert!(
            text.contains("role = \"dependency\""),
            "pre-existing role must be preserved, got:\n{text}"
        );
    }

    #[test]
    fn derive_module_name_reads_path_flake_metadata_name() {
        let tmp = tempdir().expect("tempdir");
        write_metadata(tmp.path(), "tictactoe_core");
        let src = BasecampSource::Flake(format!("path:{}#lgx", tmp.path().display()));
        let (name, note) = derive_module_name(&src).expect("derive ok");
        assert_eq!(name, "tictactoe_core");
        assert!(
            note.is_none(),
            "no assumption note when metadata is present"
        );
    }

    #[test]
    fn derive_module_name_path_flake_without_metadata_errors() {
        let tmp = tempdir().expect("tempdir");
        let src = BasecampSource::Flake(format!("path:{}#lgx", tmp.path().display()));
        let err = derive_module_name(&src).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("metadata.json"),
            "expected metadata.json in error, got: {msg}"
        );
    }

    #[test]
    fn derive_module_name_lgx_file_reads_sibling_metadata() {
        let tmp = tempdir().expect("tempdir");
        write_metadata(tmp.path(), "my_module");
        let lgx = tmp.path().join("my_module.lgx");
        fs::write(&lgx, b"stub").expect("write lgx");
        let src = BasecampSource::Path(lgx.display().to_string());
        let (name, note) = derive_module_name(&src).expect("derive ok");
        assert_eq!(name, "my_module");
        assert!(note.is_none());
    }

    #[test]
    fn derive_module_name_lgx_file_without_metadata_falls_back_to_stem_with_note() {
        let tmp = tempdir().expect("tempdir");
        let lgx = tmp.path().join("stem_only.lgx");
        fs::write(&lgx, b"stub").expect("write lgx");
        let src = BasecampSource::Path(lgx.display().to_string());
        let (name, note) = derive_module_name(&src).expect("derive ok");
        assert_eq!(name, "stem_only");
        let note = note.expect("expected assumption note on stem fallback");
        assert_eq!(note.inferred_name, "stem_only");
        assert!(note.flake_ref.contains("stem_only.lgx"));
    }

    #[test]
    fn derive_module_name_github_strips_logos_prefix_and_snake_cases() {
        let src =
            BasecampSource::Flake("github:logos-co/logos-delivery-module/abc123#lgx".to_string());
        let (name, note) = derive_module_name(&src).expect("derive ok");
        assert_eq!(name, "delivery_module");
        let note = note.expect("assumption note expected for github flake");
        assert_eq!(note.inferred_name, "delivery_module");
    }

    #[test]
    fn derive_module_name_github_non_logos_prefix_replaces_dashes_only() {
        let src = BasecampSource::Flake("github:jimmy/claw-module/def#lgx".to_string());
        let (name, note) = derive_module_name(&src).expect("derive ok");
        assert_eq!(name, "claw_module");
        assert!(note.is_some());
    }

    #[test]
    fn derive_module_name_lowercases_mixed_case_metadata_name() {
        let tmp = tempdir().expect("tempdir");
        write_metadata(tmp.path(), "TicTacToe");
        let src = BasecampSource::Flake(format!("path:{}#lgx", tmp.path().display()));
        let (name, _) = derive_module_name(&src).expect("derive ok");
        assert_eq!(name, "tictactoe", "name must be lowercased");
    }

    #[test]
    fn derive_module_name_lowercases_github_derivation() {
        let src = BasecampSource::Flake("github:logos-co/Logos-Foo/abc#lgx".to_string());
        let (name, note) = derive_module_name(&src).expect("derive ok");
        assert_eq!(name, "foo");
        assert_eq!(
            note.expect("note").inferred_name,
            "foo",
            "assumption note reports the normalized name, not the raw slug"
        );
    }

    #[test]
    fn derive_module_name_rejects_section_header_injection() {
        let tmp = tempdir().expect("tempdir");
        write_metadata(
            tmp.path(),
            // Section-header injection attempt — `]`, `[`, `\n` all forbidden.
            "x]\\n[basecamp.modules.evil",
        );
        let src = BasecampSource::Flake(format!("path:{}#lgx", tmp.path().display()));
        let err = derive_module_name(&src).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("module name") || msg.contains("invalid"),
            "error should flag the invalid name: {msg}"
        );
    }

    #[test]
    fn derive_module_name_rejects_real_newline_in_metadata_name() {
        let tmp = tempdir().expect("tempdir");
        // JSON string with an embedded real newline (serde_json parses \n as newline).
        fs::write(
            tmp.path().join("metadata.json"),
            "{\"name\": \"evil\\n[basecamp.modules.attacker\", \"dependencies\": []}",
        )
        .expect("write metadata");
        let src = BasecampSource::Flake(format!("path:{}#lgx", tmp.path().display()));
        let err = derive_module_name(&src).unwrap_err();
        assert!(err.to_string().contains("module name"), "{err}");
    }

    #[test]
    fn derive_module_name_rejects_path_traversal_name() {
        let tmp = tempdir().expect("tempdir");
        write_metadata(tmp.path(), "../evil");
        let src = BasecampSource::Flake(format!("path:{}#lgx", tmp.path().display()));
        let err = derive_module_name(&src).unwrap_err();
        assert!(err.to_string().contains("module name"), "{err}");
    }

    #[test]
    fn derive_module_name_rejects_path_separator_name() {
        let tmp = tempdir().expect("tempdir");
        write_metadata(tmp.path(), "a/b");
        let src = BasecampSource::Flake(format!("path:{}#lgx", tmp.path().display()));
        let err = derive_module_name(&src).unwrap_err();
        assert!(err.to_string().contains("module name"), "{err}");
    }

    #[test]
    fn derive_module_name_rejects_leading_dash() {
        let tmp = tempdir().expect("tempdir");
        write_metadata(tmp.path(), "-x");
        let src = BasecampSource::Flake(format!("path:{}#lgx", tmp.path().display()));
        let err = derive_module_name(&src).unwrap_err();
        assert!(err.to_string().contains("module name"), "{err}");
    }

    #[test]
    fn derive_module_name_rejects_whitespace_in_name() {
        let tmp = tempdir().expect("tempdir");
        write_metadata(tmp.path(), "has space");
        let src = BasecampSource::Flake(format!("path:{}#lgx", tmp.path().display()));
        let err = derive_module_name(&src).unwrap_err();
        assert!(err.to_string().contains("module name"), "{err}");
    }

    #[test]
    fn derive_module_name_accepts_valid_snake_case() {
        let tmp = tempdir().expect("tempdir");
        write_metadata(tmp.path(), "tictactoe_solo_ai");
        let src = BasecampSource::Flake(format!("path:{}#lgx", tmp.path().display()));
        let (name, _) = derive_module_name(&src).expect("derive ok");
        assert_eq!(name, "tictactoe_solo_ai");
    }

    #[test]
    fn assumption_note_line_contains_flake_ref_and_inferred_name() {
        let note = AssumptionNote {
            flake_ref: "github:logos-co/logos-foo/abc#lgx".to_string(),
            inferred_name: "foo".to_string(),
        };
        let line = assumption_note_line(&note);
        assert!(
            line.contains("github:logos-co/logos-foo/abc#lgx") && line.contains("foo"),
            "note line missing ref or name: {line}"
        );
        assert!(
            line.contains("scaffold.toml"),
            "note line should point user at scaffold.toml: {line}"
        );
    }

    #[test]
    fn swap_flake_attr_rewrites_matching_suffix() {
        assert_eq!(
            swap_flake_attr("path:/abs/foo#lgx", "lgx", "lgx-portable"),
            "path:/abs/foo#lgx-portable"
        );
        assert_eq!(
            swap_flake_attr(
                "github:logos-co/logos-delivery-module/1.0.0#lgx",
                "lgx",
                "lgx-portable"
            ),
            "github:logos-co/logos-delivery-module/1.0.0#lgx-portable"
        );
    }

    #[test]
    fn swap_flake_attr_leaves_non_matching_attr_untouched() {
        assert_eq!(
            swap_flake_attr("path:/abs#lgx-dev", "lgx", "lgx-portable"),
            "path:/abs#lgx-dev"
        );
        assert_eq!(
            swap_flake_attr("path:/abs", "lgx", "lgx-portable"),
            "path:/abs"
        );
    }

    // ---- doctor helpers (github_ref_part_label, infer_module_name_from_flake_ref) ----

    #[test]
    fn github_ref_part_label_recognizes_commit() {
        assert_eq!(
            github_ref_part_label("github:owner/repo/a746cdbc521f72ee22c5a4856fd17a9802bb9d69#lgx"),
            Some("commit a746cdbc521f".to_string())
        );
    }

    #[test]
    fn github_ref_part_label_recognizes_tag() {
        assert_eq!(
            github_ref_part_label("github:logos-co/logos-delivery-module/1.0.0#lgx"),
            Some("tag 1.0.0".to_string())
        );
        assert_eq!(
            github_ref_part_label("github:logos-co/logos-delivery-module/tutorial-v1#lgx"),
            Some("tag tutorial-v1".to_string())
        );
    }

    #[test]
    fn github_ref_part_label_returns_none_for_non_github() {
        assert_eq!(github_ref_part_label("path:/abs/sub#lgx"), None);
        assert_eq!(github_ref_part_label("git+https://example#lgx"), None);
    }

    #[test]
    fn infer_module_name_from_github() {
        assert_eq!(
            infer_module_name_from_flake_ref("github:logos-co/logos-delivery-module/1.0.0#lgx"),
            Some("delivery_module".to_string())
        );
    }

    #[test]
    fn infer_module_name_from_path() {
        assert_eq!(
            infer_module_name_from_flake_ref("path:/abs/tictactoe-ui-cpp#lgx"),
            Some("tictactoe_ui_cpp".to_string())
        );
    }
}
