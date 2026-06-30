//! Caller-project LEZ and circuits pin resolution for test nodes.
//!
//! Integration tests must run against the same LEZ revision and circuits
//! release as the program workspace. The scaffold default pin is a
//! convenience for generated projects, not a correctness boundary: an
//! existing repository that pins `logos-execution-zone` itself (via
//! `[repos.lez]` in `scaffold.toml`, including a caller-provided `path`
//! checkout) must have its pins honored by every `test-node` command.
//!
//! Two ownership modes matter here:
//!
//! - **Managed cache** checkouts (`[repos.lez].path` empty) live under
//!   `<cache_root>/repos/lez/<ref>` and are scaffold-owned: `prepare` may
//!   clone, fetch, and check them out freely.
//! - **Caller-provided** checkouts (`[repos.lez].path` set, or a local
//!   directory passed via `--lez-source`) are never mutated: they are
//!   accepted when they are clean worktrees at the requested commit —
//!   regardless of whether their `origin` URL is HTTPS, SSH, or a local
//!   mirror — and rejected with a targeted error otherwise.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context};
use serde::Serialize;

use crate::circuits::{
    circuits_dir_for_project, ensure_circuits_for_project, release_triple, version_eq,
};
use crate::commands::setup::SEQUENCER_BUILD_ARGS;
use crate::constants::{
    DEFAULT_CIRCUITS_VERSION, DEFAULT_LEZ, LEZ_SOURCE, LOGOS_BLOCKCHAIN_CIRCUITS_ENV,
    SEQUENCER_BIN_REL_PATH,
};
use crate::model::{CheckStatus, Project};
use crate::process::run_checked;
use crate::project::resolve_cache_root;
use crate::repo::{
    ensure_pin_exists, git_clean, git_head_sha, sync_repo_to_pin_at_path_with_opts, RepoSyncOptions,
};
use crate::DynResult;

/// CLI/API overrides for pin resolution. Every `None` falls back to the
/// caller project's `scaffold.toml`, then to the scaffold defaults.
#[derive(Clone, Debug, Default)]
pub struct PinOverrides {
    /// LEZ clone URL or local checkout directory.
    pub lez_source: Option<String>,
    /// LEZ git ref (SHA, tag, or branch).
    pub lez_ref: Option<String>,
}

/// Which layer supplied a resolved pin value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PinOrigin {
    /// Explicit `--lez-source` / `--lez-ref` / `--circuits-version` override.
    CliOverride,
    /// The caller project's `scaffold.toml`.
    ProjectConfig,
    /// Scaffold's built-in default.
    ScaffoldDefault,
}

/// Who owns (and may mutate) the LEZ checkout test nodes build from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckoutOwnership {
    /// Scaffold-managed cache checkout; `prepare` may clone/fetch/checkout.
    ManagedCache,
    /// Caller-provided checkout; never reset, force-checked-out, or
    /// otherwise destructively modified.
    CallerProvided,
}

/// The LEZ and circuits pins every test-node command will use, with the
/// origin of each value and the resolved on-disk locations.
#[derive(Clone, Debug, Serialize)]
pub struct TestNodePins {
    pub lez_source: String,
    pub lez_source_origin: PinOrigin,
    /// The requested ref (SHA, tag, or branch) before resolution.
    pub lez_ref: String,
    pub lez_ref_origin: PinOrigin,
    /// 40-hex commit the ref resolves to in the checkout, when the checkout
    /// exists locally. `None` until `prepare` has materialised it.
    pub lez_resolved_commit: Option<String>,
    /// LEZ checkout directory test-node commands will build from.
    pub lez_checkout: PathBuf,
    pub checkout_ownership: CheckoutOwnership,
    /// Standalone sequencer binary inside the checkout.
    pub sequencer_binary: PathBuf,
    pub circuits_version: String,
    pub circuits_version_origin: PinOrigin,
    /// Circuits release directory for the selected version.
    pub circuits_path: PathBuf,
}

/// Result of `test-node prepare`: pin-resolved, built, and verified
/// artefacts.
#[derive(Clone, Debug, Serialize)]
pub struct PreparedTestNode {
    /// LEZ checkout the sequencer was built from.
    pub checkout: PathBuf,
    pub checkout_ownership: CheckoutOwnership,
    /// Resolved 40-hex LEZ commit the checkout sits at.
    pub lez_commit: String,
    /// Standalone sequencer binary.
    pub sequencer_binary: PathBuf,
    pub circuits_version: String,
    /// Circuits release directory exported to spawned nodes.
    pub circuits_path: PathBuf,
}

/// Failure categories the test-node doctor distinguishes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TestNodeCheckCategory {
    UnsupportedPlatform,
    PinDrift,
    CheckoutMissing,
    MismatchedCommit,
    DirtyCheckout,
    MissingBinary,
    MissingCircuits,
}

#[derive(Clone, Debug, Serialize)]
pub struct TestNodeCheck {
    pub category: TestNodeCheckCategory,
    pub status: CheckStatus,
    pub name: String,
    pub detail: String,
    pub remediation: Option<String>,
}

/// Structured `test-node doctor` report.
#[derive(Clone, Debug, Serialize)]
pub struct TestNodeDoctorReport {
    /// `false` when any check failed.
    pub ok: bool,
    pub pins: TestNodePins,
    pub checks: Vec<TestNodeCheck>,
}

/// Resolve the LEZ and circuits pins test-node commands will use for
/// `project`, applying `overrides` first, then the project's
/// `scaffold.toml`, then scaffold defaults. Read-only: nothing is cloned,
/// fetched, built, or downloaded.
pub fn resolve_test_node_pins(
    project: &Project,
    overrides: &PinOverrides,
) -> DynResult<TestNodePins> {
    resolve_pins_with_cache_root(project, overrides, None)
}

fn resolve_pins_with_cache_root(
    project: &Project,
    overrides: &PinOverrides,
    cache_root_override: Option<&Path>,
) -> DynResult<TestNodePins> {
    let (lez_source, lez_source_origin) = match &overrides.lez_source {
        Some(source) => (source.clone(), PinOrigin::CliOverride),
        None if !project.config.lez.source.is_empty() => {
            (project.config.lez.source.clone(), PinOrigin::ProjectConfig)
        }
        None => (LEZ_SOURCE.to_string(), PinOrigin::ScaffoldDefault),
    };

    let (lez_ref, lez_ref_origin) = match &overrides.lez_ref {
        Some(reference) => (reference.clone(), PinOrigin::CliOverride),
        None if !project.config.lez.pin.is_empty() => {
            (project.config.lez.pin.clone(), PinOrigin::ProjectConfig)
        }
        None => (DEFAULT_LEZ.sha.to_string(), PinOrigin::ScaffoldDefault),
    };

    // Circuits are a project-level dependency (`[circuits].version`), so the
    // configured value is the single source of truth: `pins`, `prepare`, and
    // `start` all resolve and provision it the same way. There is deliberately
    // no CLI override — that would reintroduce prepare/start divergence.
    let (circuits_version, circuits_version_origin) =
        if !version_eq(&project.config.circuits.version, DEFAULT_CIRCUITS_VERSION) {
            (
                project.config.circuits.version.clone(),
                PinOrigin::ProjectConfig,
            )
        } else {
            (
                DEFAULT_CIRCUITS_VERSION.to_string(),
                PinOrigin::ScaffoldDefault,
            )
        };

    let cache_root = match cache_root_override {
        Some(root) => root.to_path_buf(),
        None => resolve_cache_root(project)?.0,
    };

    // A local directory passed as the source IS the checkout — caller-owned.
    let source_as_local_checkout = local_checkout_from_source(project, &lez_source);

    let (lez_checkout, checkout_ownership) = if let Some(dir) = source_as_local_checkout {
        (dir, CheckoutOwnership::CallerProvided)
    } else if overrides.lez_source.is_none()
        && overrides.lez_ref.is_none()
        && !project.config.lez.path.is_empty()
    {
        // `[repos.lez].path` checkout from scaffold.toml (vendored or
        // user-overridden) — caller-owned.
        let path = PathBuf::from(&project.config.lez.path);
        let absolute = if path.is_absolute() {
            path
        } else {
            project.root.join(path)
        };
        (absolute, CheckoutOwnership::CallerProvided)
    } else {
        // Managed cache checkout, keyed by the requested ref so different
        // pins coexist.
        (
            cache_root.join("repos").join("lez").join(&lez_ref),
            CheckoutOwnership::ManagedCache,
        )
    };

    // Resolve the ref to a commit only when the checkout already exists;
    // `pins` stays read-only.
    let lez_resolved_commit = if lez_checkout.join(".git").exists() {
        ensure_pin_exists(&lez_checkout, &lez_source, &lez_ref, "lez").ok()
    } else {
        None
    };

    let circuits_path = circuits_dir_for_project(project);

    Ok(TestNodePins {
        sequencer_binary: lez_checkout.join(SEQUENCER_BIN_REL_PATH),
        lez_source,
        lez_source_origin,
        lez_ref,
        lez_ref_origin,
        lez_resolved_commit,
        lez_checkout,
        checkout_ownership,
        circuits_version,
        circuits_version_origin,
        circuits_path,
    })
}

/// When `source` denotes an existing local git checkout (plain path or
/// `file://` URL), return its directory. Relative paths resolve against the
/// project root.
fn local_checkout_from_source(project: &Project, source: &str) -> Option<PathBuf> {
    let raw = source.strip_prefix("file://").unwrap_or(source);
    if raw.contains("://") || raw.starts_with("git@") {
        return None;
    }
    let path = PathBuf::from(raw);
    let absolute = if path.is_absolute() {
        path
    } else {
        project.root.join(path)
    };
    if absolute.join(".git").exists() {
        Some(absolute)
    } else {
        None
    }
}

/// Resolve pins, materialise the LEZ checkout and circuits release, and
/// build the standalone sequencer for those pins.
///
/// Managed cache checkouts are cloned/fetched/checked out as needed.
/// Caller-provided checkouts are only validated: they must be clean
/// worktrees with the requested commit checked out, and are never reset or
/// force-checked-out. The origin URL form (HTTPS, SSH, local mirror) is not
/// part of the validation.
pub fn prepare_test_node(
    project: &Project,
    overrides: &PinOverrides,
    cache_root: Option<&Path>,
) -> DynResult<PreparedTestNode> {
    let pins = resolve_pins_with_cache_root(project, overrides, cache_root)?;

    let lez_commit = match pins.checkout_ownership {
        CheckoutOwnership::ManagedCache => {
            sync_repo_to_pin_at_path_with_opts(
                &pins.lez_checkout,
                &pins.lez_source,
                &pins.lez_ref,
                "lez",
                RepoSyncOptions::auto_reclone_cache_repo(),
            )?;
            git_head_sha(&pins.lez_checkout)?
        }
        CheckoutOwnership::CallerProvided => {
            validate_caller_checkout(&pins.lez_checkout, &pins.lez_ref)?
        }
    };

    // Provision circuits into the project's configured install dir and export
    // `LOGOS_BLOCKCHAIN_CIRCUITS` — exactly what `start` does, so `prepare`
    // warms the same tree `start` will use (not a separate cache-root copy).
    // Needed for the sequencer build below and any node spawned in this process.
    ensure_circuits_for_project(project)?;
    let circuits_path = circuits_dir_for_project(project);

    let mut build_cmd = Command::new("cargo");
    build_cmd
        .current_dir(&pins.lez_checkout)
        .args(SEQUENCER_BUILD_ARGS);
    run_checked(&mut build_cmd, "build sequencer_service (standalone)")?;

    if !pins.sequencer_binary.exists() {
        bail!(
            "sequencer build succeeded but binary is missing at {}",
            pins.sequencer_binary.display()
        );
    }

    Ok(PreparedTestNode {
        checkout: pins.lez_checkout.clone(),
        checkout_ownership: pins.checkout_ownership,
        lez_commit,
        sequencer_binary: pins.sequencer_binary.clone(),
        circuits_version: pins.circuits_version.clone(),
        circuits_path,
    })
}

/// Validate a caller-provided checkout without mutating it: it must be a git
/// worktree, contain the requested ref, sit at that commit, and be clean.
/// Returns the resolved commit.
fn validate_caller_checkout(checkout: &Path, requested_ref: &str) -> DynResult<String> {
    if !checkout.join(".git").exists() {
        bail!(
            "caller-provided LEZ checkout at {} is not a git repository",
            checkout.display()
        );
    }

    let resolved = ensure_pin_exists(checkout, "<caller-provided>", requested_ref, "lez")
        .with_context(|| {
            format!(
                "caller-provided checkout at {} does not contain the requested ref `{requested_ref}`",
                checkout.display()
            )
        })?;

    let head = git_head_sha(checkout)?;
    if head != resolved {
        bail!(
            "caller-provided checkout at {} is at commit {head}, but the requested ref \
             `{requested_ref}` resolves to {resolved}.\n\
             Scaffold will not check out or reset a caller-provided worktree; switch it to the \
             requested commit yourself and retry.",
            checkout.display()
        );
    }

    if !git_clean(checkout)? {
        bail!(
            "caller-provided checkout at {} has uncommitted changes.\n\
             Scaffold will not reset a caller-provided worktree; commit or stash the changes \
             and retry.",
            checkout.display()
        );
    }

    Ok(resolved)
}

/// Run the test-node health checks for `project` and report each failure
/// class separately: unsupported platform, pin drift, missing/dirty/
/// mismatched checkouts, missing sequencer binary, missing circuits.
/// Read-only: nothing is mutated, built, or downloaded.
pub fn doctor_test_node(project: &Project) -> DynResult<TestNodeDoctorReport> {
    let pins = resolve_test_node_pins(project, &PinOverrides::default())?;
    let mut checks = Vec::new();

    // Platform support: a circuits release must exist for this OS/arch — unless
    // the user supplies their own circuits via `LOGOS_BLOCKCHAIN_CIRCUITS`,
    // the documented escape hatch on otherwise-unsupported platforms.
    let circuits_env_override =
        std::env::var_os(LOGOS_BLOCKCHAIN_CIRCUITS_ENV).is_some_and(|v| !v.is_empty());
    if release_triple().is_ok() || circuits_env_override {
        checks.push(TestNodeCheck {
            category: TestNodeCheckCategory::UnsupportedPlatform,
            status: CheckStatus::Pass,
            name: "platform supported".to_string(),
            detail: format!("{}/{}", std::env::consts::OS, std::env::consts::ARCH),
            remediation: None,
        });
    } else {
        checks.push(TestNodeCheck {
            category: TestNodeCheckCategory::UnsupportedPlatform,
            status: CheckStatus::Fail,
            name: "platform supported".to_string(),
            detail: format!("{:#}", release_triple().unwrap_err()),
            remediation: Some(format!(
                "Set {LOGOS_BLOCKCHAIN_CIRCUITS_ENV} to a populated circuits checkout"
            )),
        });
    }

    // Pin drift vs the scaffold default — informational, not an error: the
    // caller project's pin is authoritative for test nodes.
    let drift = pins.lez_ref != DEFAULT_LEZ.sha && pins.lez_ref != DEFAULT_LEZ.tag;
    checks.push(TestNodeCheck {
        category: TestNodeCheckCategory::PinDrift,
        status: if drift {
            CheckStatus::Warn
        } else {
            CheckStatus::Pass
        },
        name: "lez pin vs scaffold default".to_string(),
        detail: format!(
            "configured ref={} (origin: {:?}), scaffold default={} ({})",
            pins.lez_ref, pins.lez_ref_origin, DEFAULT_LEZ.sha, DEFAULT_LEZ.tag
        ),
        remediation: if drift {
            Some(
                "Informational: test nodes follow the project pin. Align with the scaffold \
                 default only if you want the stock-tested LEZ revision."
                    .to_string(),
            )
        } else {
            None
        },
    });

    // Checkout state.
    if !pins.lez_checkout.join(".git").exists() {
        checks.push(TestNodeCheck {
            category: TestNodeCheckCategory::CheckoutMissing,
            status: CheckStatus::Fail,
            name: "lez checkout present".to_string(),
            detail: format!("no git checkout at {}", pins.lez_checkout.display()),
            remediation: Some("Run `lgs test-node prepare` (or `lgs setup`)".to_string()),
        });
    } else {
        match &pins.lez_resolved_commit {
            None => checks.push(TestNodeCheck {
                category: TestNodeCheckCategory::MismatchedCommit,
                status: CheckStatus::Fail,
                name: "requested ref present in checkout".to_string(),
                detail: format!(
                    "ref `{}` does not resolve in {}",
                    pins.lez_ref,
                    pins.lez_checkout.display()
                ),
                remediation: Some(match pins.checkout_ownership {
                    CheckoutOwnership::ManagedCache => {
                        "Run `lgs test-node prepare` to fetch the pinned commit".to_string()
                    }
                    CheckoutOwnership::CallerProvided => {
                        "Fetch the pinned commit into the caller-provided checkout".to_string()
                    }
                }),
            }),
            Some(resolved) => {
                let head = git_head_sha(&pins.lez_checkout).unwrap_or_default();
                let at_commit = head == *resolved;
                checks.push(TestNodeCheck {
                    category: TestNodeCheckCategory::MismatchedCommit,
                    status: if at_commit {
                        CheckStatus::Pass
                    } else {
                        CheckStatus::Fail
                    },
                    name: "checkout at pinned commit".to_string(),
                    detail: format!("HEAD={head} expected={resolved}"),
                    remediation: if at_commit {
                        None
                    } else {
                        Some(match pins.checkout_ownership {
                            CheckoutOwnership::ManagedCache => {
                                "Run `lgs test-node prepare` to re-sync the managed checkout"
                                    .to_string()
                            }
                            CheckoutOwnership::CallerProvided => {
                                "Switch the caller-provided checkout to the pinned commit \
                                 (scaffold never checks out caller worktrees)"
                                    .to_string()
                            }
                        })
                    },
                });

                let clean = git_clean(&pins.lez_checkout).unwrap_or(false);
                checks.push(TestNodeCheck {
                    category: TestNodeCheckCategory::DirtyCheckout,
                    status: if clean {
                        CheckStatus::Pass
                    } else {
                        match pins.checkout_ownership {
                            // prepare auto-reclones managed cache repos.
                            CheckoutOwnership::ManagedCache => CheckStatus::Warn,
                            CheckoutOwnership::CallerProvided => CheckStatus::Fail,
                        }
                    },
                    name: "checkout clean".to_string(),
                    detail: if clean {
                        "worktree clean".to_string()
                    } else {
                        format!("uncommitted changes in {}", pins.lez_checkout.display())
                    },
                    remediation: if clean {
                        None
                    } else {
                        Some(match pins.checkout_ownership {
                            CheckoutOwnership::ManagedCache => {
                                "Run `lgs test-node prepare` to re-sync the managed checkout"
                                    .to_string()
                            }
                            CheckoutOwnership::CallerProvided => {
                                "Commit or stash the changes (scaffold never resets caller \
                                 worktrees)"
                                    .to_string()
                            }
                        })
                    },
                });
            }
        }
    }

    // Sequencer binary.
    let bin_present = pins.sequencer_binary.exists();
    checks.push(TestNodeCheck {
        category: TestNodeCheckCategory::MissingBinary,
        status: if bin_present {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        },
        name: "standalone sequencer binary".to_string(),
        detail: pins.sequencer_binary.display().to_string(),
        remediation: if bin_present {
            None
        } else {
            Some("Run `lgs test-node prepare` to build it".to_string())
        },
    });

    // Circuits release.
    let circuits_present = pins
        .circuits_path
        .join("pol/verification_key.json")
        .is_file();
    checks.push(TestNodeCheck {
        category: TestNodeCheckCategory::MissingCircuits,
        status: if circuits_present {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        },
        name: format!("circuits release v{}", pins.circuits_version),
        detail: pins.circuits_path.display().to_string(),
        remediation: if circuits_present {
            None
        } else {
            Some("Run `lgs test-node prepare` to download it".to_string())
        },
    });

    let ok = !checks.iter().any(|check| check.status == CheckStatus::Fail);
    Ok(TestNodeDoctorReport { ok, pins, checks })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::process::Command;

    use tempfile::tempdir;

    use super::*;
    use crate::model::{
        Config, FrameworkConfig, FrameworkIdlConfig, LocalnetConfig, RepoRef, RunConfig,
    };

    fn fixture_project(root: &Path, lez: RepoRef) -> Project {
        Project {
            root: root.to_path_buf(),
            config: Config {
                version: "0.2.0".into(),
                cache_root: ".scaffold/cache".into(),
                lez,
                spel: RepoRef::default(),
                basecamp_repo: None,
                lgpm_repo: None,
                wallet_home_dir: ".scaffold/wallet".into(),
                circuits: crate::model::CircuitsConfig::default(),
                framework: FrameworkConfig {
                    kind: "default".into(),
                    version: "0.1.0".into(),
                    idl: FrameworkIdlConfig {
                        spec: String::new(),
                        path: String::new(),
                    },
                },
                localnet: LocalnetConfig::default(),
                modules: std::collections::BTreeMap::new(),
                basecamp: None,
                run: RunConfig::default(),
            },
        }
    }

    fn init_git_repo(path: &Path) -> String {
        fs::create_dir_all(path).unwrap();
        for args in [
            vec!["init", "--quiet", "--initial-branch=main"],
            vec!["config", "user.email", "t@example.com"],
            vec!["config", "user.name", "test"],
            vec!["config", "commit.gpgsign", "false"],
        ] {
            assert!(Command::new("git")
                .args(&args)
                .current_dir(path)
                .status()
                .unwrap()
                .success());
        }
        fs::write(path.join("README.md"), "seed").unwrap();
        assert!(Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .status()
            .unwrap()
            .success());
        assert!(Command::new("git")
            .args(["commit", "--quiet", "-m", "seed"])
            .current_dir(path)
            .status()
            .unwrap()
            .success());
        let out = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(path)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    #[test]
    fn pins_prefer_overrides_then_config_then_defaults() {
        let temp = tempdir().unwrap();
        let project = fixture_project(
            temp.path(),
            RepoRef {
                source: "https://example.com/custom-lez.git".into(),
                pin: "abc123".into(),
                ..Default::default()
            },
        );

        // Config wins over default.
        let pins = resolve_test_node_pins(&project, &PinOverrides::default()).unwrap();
        assert_eq!(pins.lez_source, "https://example.com/custom-lez.git");
        assert_eq!(pins.lez_source_origin, PinOrigin::ProjectConfig);
        assert_eq!(pins.lez_ref, "abc123");
        assert_eq!(pins.lez_ref_origin, PinOrigin::ProjectConfig);
        assert_eq!(pins.circuits_version_origin, PinOrigin::ScaffoldDefault);
        assert_eq!(pins.checkout_ownership, CheckoutOwnership::ManagedCache);
        assert!(pins
            .lez_checkout
            .ends_with(PathBuf::from("repos/lez/abc123")));

        // Override wins over config.
        let pins = resolve_test_node_pins(
            &project,
            &PinOverrides {
                lez_ref: Some("v9.9.9".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(pins.lez_ref, "v9.9.9");
        assert_eq!(pins.lez_ref_origin, PinOrigin::CliOverride);
    }

    #[test]
    fn circuits_version_uses_project_config_as_source_of_truth() {
        let temp = tempdir().unwrap();
        let mut project = fixture_project(temp.path(), RepoRef::default());
        // Circuits version is project config (there is no CLI override): a
        // non-default `[circuits].version` is the single pin that `pins`,
        // `prepare`, and `start` all resolve and provision.
        project.config.circuits.version = "0.9.9".into();

        let pins = resolve_test_node_pins(&project, &PinOverrides::default()).unwrap();
        assert_eq!(pins.circuits_version, "0.9.9");
        assert_eq!(pins.circuits_version_origin, PinOrigin::ProjectConfig);
    }

    #[test]
    fn pins_fall_back_to_scaffold_defaults() {
        let temp = tempdir().unwrap();
        let project = fixture_project(temp.path(), RepoRef::default());
        let pins = resolve_test_node_pins(&project, &PinOverrides::default()).unwrap();
        assert_eq!(pins.lez_source, LEZ_SOURCE);
        assert_eq!(pins.lez_source_origin, PinOrigin::ScaffoldDefault);
        assert_eq!(pins.lez_ref, DEFAULT_LEZ.sha);
        assert_eq!(pins.lez_ref_origin, PinOrigin::ScaffoldDefault);
    }

    #[test]
    fn caller_path_in_config_is_caller_provided() {
        let temp = tempdir().unwrap();
        let checkout = temp.path().join("vendored-lez");
        let head = init_git_repo(&checkout);

        let project = fixture_project(
            temp.path(),
            RepoRef {
                source: "git@example.com:lez.git".into(),
                pin: head.clone(),
                path: checkout.display().to_string(),
                ..Default::default()
            },
        );

        let pins = resolve_test_node_pins(&project, &PinOverrides::default()).unwrap();
        assert_eq!(pins.checkout_ownership, CheckoutOwnership::CallerProvided);
        assert_eq!(pins.lez_checkout, checkout);
        assert_eq!(pins.lez_resolved_commit.as_deref(), Some(head.as_str()));
    }

    #[test]
    fn local_source_directory_is_caller_provided() {
        let temp = tempdir().unwrap();
        let checkout = temp.path().join("local-lez");
        init_git_repo(&checkout);
        let project = fixture_project(temp.path(), RepoRef::default());

        let pins = resolve_test_node_pins(
            &project,
            &PinOverrides {
                lez_source: Some(checkout.display().to_string()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(pins.checkout_ownership, CheckoutOwnership::CallerProvided);
        assert_eq!(pins.lez_checkout, checkout);
    }

    #[test]
    fn validate_caller_checkout_accepts_clean_checkout_at_commit() {
        let temp = tempdir().unwrap();
        let checkout = temp.path().join("lez");
        let head = init_git_repo(&checkout);
        let resolved = validate_caller_checkout(&checkout, &head).unwrap();
        assert_eq!(resolved, head);
    }

    #[test]
    fn validate_caller_checkout_rejects_dirty_worktree() {
        let temp = tempdir().unwrap();
        let checkout = temp.path().join("lez");
        let head = init_git_repo(&checkout);
        fs::write(checkout.join("README.md"), "dirty").unwrap();

        let err = validate_caller_checkout(&checkout, &head).unwrap_err();
        assert!(err.to_string().contains("uncommitted changes"), "{err}");
    }

    #[test]
    fn validate_caller_checkout_rejects_missing_ref() {
        let temp = tempdir().unwrap();
        let checkout = temp.path().join("lez");
        init_git_repo(&checkout);

        let err = validate_caller_checkout(&checkout, "0000000000000000000000000000000000000000")
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("does not contain the requested ref"),
            "{err}"
        );
    }

    #[test]
    fn validate_caller_checkout_rejects_wrong_head() {
        let temp = tempdir().unwrap();
        let checkout = temp.path().join("lez");
        let first = init_git_repo(&checkout);
        // Second commit moves HEAD away from `first`.
        fs::write(checkout.join("second.txt"), "x").unwrap();
        for args in [vec!["add", "."], vec!["commit", "--quiet", "-m", "second"]] {
            assert!(Command::new("git")
                .args(&args)
                .current_dir(&checkout)
                .status()
                .unwrap()
                .success());
        }

        let err = validate_caller_checkout(&checkout, &first).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("will not check out or reset"), "{message}");
    }

    #[test]
    fn doctor_reports_missing_checkout_and_binary() {
        let temp = tempdir().unwrap();
        let project = fixture_project(temp.path(), RepoRef::default());

        let report = doctor_test_node(&project).unwrap();
        assert!(!report.ok);
        let categories: Vec<TestNodeCheckCategory> = report
            .checks
            .iter()
            .filter(|check| check.status == CheckStatus::Fail)
            .map(|check| check.category)
            .collect();
        assert!(categories.contains(&TestNodeCheckCategory::CheckoutMissing));
        assert!(categories.contains(&TestNodeCheckCategory::MissingBinary));
        assert!(categories.contains(&TestNodeCheckCategory::MissingCircuits));
    }

    #[test]
    fn doctor_flags_dirty_caller_checkout_as_fail() {
        let temp = tempdir().unwrap();
        let checkout = temp.path().join("lez");
        let head = init_git_repo(&checkout);
        fs::write(checkout.join("README.md"), "dirty").unwrap();

        let project = fixture_project(
            temp.path(),
            RepoRef {
                source: "ssh://git@example.com/lez.git".into(),
                pin: head,
                path: checkout.display().to_string(),
                ..Default::default()
            },
        );

        let report = doctor_test_node(&project).unwrap();
        let dirty = report
            .checks
            .iter()
            .find(|check| check.category == TestNodeCheckCategory::DirtyCheckout)
            .expect("dirty check present");
        assert_eq!(dirty.status, CheckStatus::Fail);
    }
}
