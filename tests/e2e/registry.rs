//! The scenario list, the check that it matches `.dogfooding/`, the run loop
//! and the report.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::harness::{tail, Ctx, Stop, R};
use crate::scenarios::*;

pub struct Scenario {
    pub id: &'static str,
    pub run: fn(&Ctx) -> R,
}

const fn sc(id: &'static str, run: fn(&Ctx) -> R) -> Scenario {
    Scenario { id, run }
}

/// Automated scenarios, in run order. The order follows the run plan in
/// `.dogfooding/product.md`: everything that shares a build runs on it once.
pub const SCENARIOS: &[Scenario] = &[
    // No project needed.
    sc("CLI-001", cli::help_version_unknown),
    sc("CLI-003", cli::outside_project),
    sc("DIAG-005", diag::completions),
    // Creation.
    sc("CREATE-001", create::default_project),
    sc("CREATE-003", create::templates_and_alias),
    sc("CREATE-007", create::init_adopts),
    sc("CREATE-008", create::init_migrates),
    sc("SKILLS-002", skills::init_refresh),
    sc("BCSETUP-006", basecamp::without_nix_or_setup),
    sc("BCLAUNCH-005", basecamp::paths_pure),
    sc("TNODE-001", tnode::pins),
    sc("BUILD-003", build::missing_docker_toolchain),
    sc("CREATE-004", create::vendor_deps),
    sc("CREATE-005", create::cache_isolation),
    sc("BUILD-006", build::prebuilt_sequencer),
    // The main project: one setup, one build, one localnet, one deploy.
    sc("BUILD-001", build::setup_builds_toolchain),
    sc("SKILLS-001", skills::new_projects_ship_skills),
    sc("CLI-006", cli::quiet_mode),
    sc("CLI-008", cli::closed_pipe),
    sc("LOCALNET-001", localnet::start_ready),
    sc("LOCALNET-002", localnet::status_and_logs),
    sc("LOCALNET-004", localnet::stop_and_survive),
    sc("WALLET-001", wallet::topup_default),
    sc("WALLET-002", wallet::list_and_passthrough),
    sc("WALLET-003", wallet::default_address),
    sc("WALLET-004", wallet::dry_run_and_target),
    sc("WALLET-007", wallet::password_override),
    sc("BUILD-005", build::local_guest_build),
    sc("BUILD-002", build::docker_build),
    sc("DEPLOY-001", deploy::deploy_all),
    sc("DEPLOY-002", deploy::deploy_one),
    sc("DEPLOY-005", deploy::unknown_or_missing),
    sc("DEPLOY-006", deploy::unreachable_sequencer),
    sc("DEPLOY-007", deploy::artefact_ranking),
    sc("RUNNER-001", runner::public_runners),
    sc("RUNNER-003", runner::bad_input),
    sc("RUNNER-004", runner::private_runner),
    sc("RUNNER-005", runner::tail_call),
    sc("DIAG-001", diag::doctor_checks),
    sc("DIAG-004", diag::report_bundle),
    sc("DIAG-006", diag::wallet_sequencer_address),
    // Disposable test nodes, and the library.
    sc("TNODE-002", tnode::prepare_and_doctor),
    sc("TNODE-003", tnode::start_inspect_stop),
    sc("TNODE-004", tnode::run_wrapper),
    sc("TNODE-005", tnode::start_options),
    sc("TNODE-006", tnode::caller_checkout),
    sc("TRPC-001", trpc::blocks_and_clock),
    sc("TRPC-002", trpc::tx_outcomes),
    sc("TRPC-003", trpc::account_and_proof_reads),
    sc("TRPC-005", trpc::seeded_state),
    sc("TRPC-006", trpc::bad_snapshots),
    sc("API-001", api::open_and_paths),
    sc("API-003", api::error_categories),
    sc("API-004", api::test_node_from_library),
    sc("TRPC-007", trpc::real_transaction),
    // These wipe the chain, so they close the main project's block.
    sc("LOCALNET-006", localnet::reset),
    sc("LOCALNET-007", localnet::start_failures),
    // A second, fresh project for the one-command inner loop.
    sc("RUN-001", run::bare_run),
    sc("RUN-002", run::rerun),
    sc("RUN-003", run::hooks),
    sc("RUN-005", run::profiles),
    sc("RUN-008", run::watch),
    sc("RUN-010", run::single_program),
    sc("RUN-007", run::reset),
    // The dashboard, on the run project.
    sc("TUI-001", tui::status_agrees),
    sc("TUI-004", tui::one_job_and_confirmations),
    sc("TUI-006", tui::quit_and_no_terminal),
    sc("TUI-007", tui::rendering),
    sc("TUI-002", tui::keys_run_commands),
    // The spel template.
    sc("SPEL-001", spel::create),
    sc("SPEL-003", spel::idl_and_client),
    sc("SPEL-002", spel::setup_and_build),
    sc("SPEL-005", spel::deploy_and_invoke),
];

/// Scenarios this suite does not automate, and why. Every scenario in
/// `.dogfooding/features/` is either in [`SCENARIOS`] or here.
pub const MANUAL: &[(&str, &str)] = &[
    (
        "API-002",
        "`cargo test` already builds the docs' examples as doctests; `cargo doc` is not behaviour",
    ),
    (
        "BUILD-004",
        "byte reproducibility across a cleared BuildKit cache is the `guest-reproducibility.yml` workflow",
    ),
    ("BCSETUP-001", BASECAMP_APP),
    ("BCSETUP-002", BASECAMP_APP),
    ("BCSETUP-004", BASECAMP_APP),
    (
        "BCSETUP-007",
        "needs Nix builds of lgpm and of two module projects (tens of minutes without the Logos binary cache)",
    ),
    ("BCMOD-001", BASECAMP_APP),
    ("BCMOD-002", BASECAMP_APP),
    ("BCMOD-003", BASECAMP_APP),
    ("BCMOD-005", BASECAMP_APP),
    ("BCMOD-007", BASECAMP_APP),
    ("BCLAUNCH-001", BASECAMP_GUI),
    ("BCLAUNCH-002", BASECAMP_GUI),
    ("BCLAUNCH-003", BASECAMP_GUI),
    ("BCLAUNCH-006", BASECAMP_GUI),
    ("BCLAUNCH-007", BASECAMP_GUI),
];

const BASECAMP_APP: &str =
    "needs the basecamp app built with Nix (about 5 GB of RAM or the Logos binary cache) and a module project";
const BASECAMP_GUI: &str =
    "needs the basecamp app and a graphical session; the checks are on windows and a second profile";

#[derive(Debug, Clone)]
pub struct Meta {
    pub title: String,
    pub priority: String,
}

/// Scenario IDs, titles and priorities from the feature files' frontmatter.
pub fn context(repo: &Path) -> BTreeMap<String, Meta> {
    let mut map = BTreeMap::new();
    let dir = repo.join(".dogfooding/features");
    let Ok(entries) = fs::read_dir(&dir) else {
        return map;
    };
    for entry in entries.flatten() {
        let text = fs::read_to_string(entry.path()).unwrap_or_default();
        let mut id: Option<String> = None;
        let mut title = String::new();
        let mut fences = 0;
        for line in text.lines() {
            if line == "---" {
                fences += 1;
                if fences == 2 {
                    break;
                }
                continue;
            }
            if let Some(v) = line.strip_prefix("  - id: ") {
                id = Some(v.trim().to_string());
            } else if let Some(v) = line.strip_prefix("    title: ") {
                title = v.trim().to_string();
            } else if let Some(v) = line.strip_prefix("    priority: ") {
                if let Some(id) = id.take() {
                    map.insert(
                        id,
                        Meta {
                            title: std::mem::take(&mut title),
                            priority: v.trim().to_string(),
                        },
                    );
                }
            }
        }
    }
    map
}

/// Problems that make the suite and the dogfooding context disagree.
pub fn sync_problems(ctx: &BTreeMap<String, Meta>) -> Vec<String> {
    let mut problems = Vec::new();
    let mut seen = BTreeSet::new();
    for id in SCENARIOS
        .iter()
        .map(|s| s.id)
        .chain(MANUAL.iter().map(|m| m.0))
    {
        if !seen.insert(id) {
            problems.push(format!("{id} is listed twice in tests/e2e/registry.rs"));
        }
        if !ctx.contains_key(id) {
            problems.push(format!(
                "{id} is in tests/e2e/registry.rs but not in .dogfooding/features/ (retired or renamed?)"
            ));
        }
    }
    for id in ctx.keys() {
        if !seen.contains(id.as_str()) {
            problems.push(format!(
                "{id} is in .dogfooding/features/ but neither automated nor listed in MANUAL in tests/e2e/registry.rs"
            ));
        }
    }
    problems
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Passed,
    Failed,
    Blocked,
    Skipped,
}

impl Status {
    fn word(self) -> &'static str {
        match self {
            Status::Passed => "passed",
            Status::Failed => "failed",
            Status::Blocked => "blocked",
            Status::Skipped => "skipped",
        }
    }
}

struct Row {
    id: &'static str,
    status: Status,
    note: String,
    detail: String,
    secs: f64,
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

fn selected(id: &str, filters: &[String]) -> bool {
    filters.is_empty() || filters.iter().any(|f| id.starts_with(f.as_str()))
}

fn one_line(s: &str) -> String {
    let first = s.lines().next().unwrap_or("").trim();
    let mut t: String = first.chars().take(160).collect();
    if first.chars().count() > 160 || s.lines().count() > 1 {
        t.push('…');
    }
    t.replace('|', "\\|")
}

pub fn main() -> i32 {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if !repo.join(".dogfooding/features").is_dir() {
        // The published crate ships `tests/` without the scenario files.
        println!(
            "e2e: no .dogfooding/ next to this crate; run the suite from a repository checkout"
        );
        return 0;
    }
    let ctx = context(&repo);
    let args: Vec<String> = std::env::args().skip(1).collect();
    let list = args.iter().any(|a| a == "--list");
    // Anything starting with `-` is a libtest flag some tool passed along.
    let filters: Vec<String> = args.into_iter().filter(|a| !a.starts_with('-')).collect();

    let problems = sync_problems(&ctx);
    if !problems.is_empty() {
        eprintln!("e2e: the suite and .dogfooding/ disagree:");
        for p in &problems {
            eprintln!("  - {p}");
        }
        return 1;
    }

    if list {
        for s in SCENARIOS {
            let m = &ctx[s.id];
            println!("auto    {:<13} {}  {}", s.id, m.priority, m.title);
        }
        for (id, why) in MANUAL {
            let m = &ctx[*id];
            println!("manual  {:<13} {}  {}  [{why}]", id, m.priority, m.title);
        }
        return 0;
    }

    if std::env::var("LGS_E2E").map(|v| v != "1").unwrap_or(true) {
        println!(
            "e2e: {} scenarios automated, {} manual, in sync with .dogfooding/. Set LGS_E2E=1 to run them (see tests/e2e/README.md).",
            SCENARIOS.len(),
            MANUAL.len()
        );
        return 0;
    }

    let user_work = env_path("LGS_E2E_WORKDIR");
    let work = user_work
        .clone()
        .unwrap_or_else(|| std::env::temp_dir().join(format!("lgs-e2e-{}", std::process::id())));
    let out = env_path("LGS_E2E_OUT").unwrap_or_else(|| work.join("_report"));
    let cache = env_path("LGS_E2E_CACHE_ROOT").unwrap_or_else(|| work.join("_cache"));
    // By default the binaries cargo just built; `LGS_E2E_BIN` tests another
    // `logos-scaffold` (a release, an older build), with `lgs` next to it.
    let (lgs, alias) = match env_path("LGS_E2E_BIN") {
        Some(bin) => {
            let alias = bin.with_file_name("lgs");
            (bin, alias)
        }
        None => (
            PathBuf::from(env!("CARGO_BIN_EXE_logos-scaffold")),
            PathBuf::from(env!("CARGO_BIN_EXE_lgs")),
        ),
    };
    if !lgs.is_file() || !alias.is_file() {
        eprintln!(
            "e2e: no binaries to test at {} and {}",
            lgs.display(),
            alias.display()
        );
        return 1;
    }
    let cx = Ctx::new(
        lgs,
        alias,
        repo.clone(),
        work.clone(),
        out.clone(),
        cache.clone(),
    );
    println!("e2e: binary  {}", cx.lgs.display());
    println!("e2e: workdir {}", work.display());
    println!("e2e: cache   {}", cache.display());
    println!("e2e: report  {}", out.display());
    println!("e2e: host    {:?}", cx.caps);

    cx.begin("_startup");
    cx.stop_stale_localnets();
    let started = Instant::now();
    let mut rows: Vec<Row> = Vec::new();
    for s in SCENARIOS.iter().filter(|s| selected(s.id, &filters)) {
        cx.begin(s.id);
        let t = Instant::now();
        let result = (s.run)(&cx);
        let secs = t.elapsed().as_secs_f64();
        let notes = cx.take_notes().join("; ");
        let (status, note, detail) = match result {
            Ok(()) => (Status::Passed, notes, String::new()),
            Err(Stop::Fail(m)) => (Status::Failed, one_line(&m), m),
            Err(Stop::Blocked(m)) => (Status::Blocked, one_line(&m), m),
            Err(Stop::Skip(m)) => (Status::Skipped, one_line(&m), m),
        };
        cx.log(&format!("=== {}: {}\n{}", s.id, status.word(), detail));
        println!(
            "{:<8} {:<13} {:>6.1}s  {}",
            status.word(),
            s.id,
            secs,
            if status == Status::Passed {
                note.clone()
            } else {
                one_line(&detail)
            }
        );
        if matches!(status, Status::Failed | Status::Blocked) {
            for line in tail(&detail, 2500).lines() {
                println!("         | {line}");
            }
        }
        rows.push(Row {
            id: s.id,
            status,
            note,
            detail,
            secs,
        });
    }
    cx.localnet_down();

    let count = |st: Status| rows.iter().filter(|r| r.status == st).count();
    let summary = format!(
        "passed {}, failed {}, blocked {}, skipped {} ({} run, {} manual, {:.0}s)",
        count(Status::Passed),
        count(Status::Failed),
        count(Status::Blocked),
        count(Status::Skipped),
        rows.len(),
        MANUAL.len(),
        started.elapsed().as_secs_f64()
    );
    write_report(&cx, &ctx, &rows, &summary);
    println!("e2e: {summary}");
    println!("e2e: report at {}", out.join("report.md").display());

    let bad = count(Status::Failed) + count(Status::Blocked);
    if user_work.is_none() && std::env::var_os("LGS_E2E_KEEP").is_none() && bad == 0 {
        let _ = fs::remove_dir_all(&work);
    }
    i32::from(bad > 0)
}

fn write_report(cx: &Ctx, ctx: &BTreeMap<String, Meta>, rows: &[Row], summary: &str) {
    let version = cx
        .lgs(&cx.work, &["--version"])
        .run()
        .stdout
        .trim()
        .to_string();
    let commit = cx
        .cmd(&cx.repo, "git", &["rev-parse", "HEAD"])
        .run()
        .stdout
        .trim()
        .to_string();
    let mut md = format!(
        "# logos-scaffold end-to-end report\n\n- binary: `{version}` (repo commit `{commit}`)\n- host: {:?}\n- result: {summary}\n\n| ID | Title | Priority | Status | Time | Note |\n|---|---|---|---|---|---|\n",
        cx.caps
    );
    let mut json = Vec::new();
    for r in rows {
        let m = &ctx[r.id];
        md.push_str(&format!(
            "| {} | {} | {} | {} | {:.0}s | {} |\n",
            r.id,
            m.title,
            m.priority,
            r.status.word(),
            r.secs,
            r.note
        ));
        json.push(serde_json::json!({
            "id": r.id, "title": m.title, "priority": m.priority,
            "status": r.status.word(), "seconds": r.secs, "note": r.note, "detail": r.detail,
        }));
    }
    let failures: Vec<&Row> = rows
        .iter()
        .filter(|r| matches!(r.status, Status::Failed | Status::Blocked))
        .collect();
    if !failures.is_empty() {
        md.push_str("\n## Failed and blocked\n");
        for r in failures {
            md.push_str(&format!(
                "\n### {} — {}\n\n```text\n{}\n```\n\nFull log: `logs/{}.log`\n",
                r.id,
                r.status.word(),
                tail(&r.detail, 4000),
                r.id
            ));
        }
    }
    md.push_str("\n## Not automated\n\n| ID | Title | Priority | Why |\n|---|---|---|---|\n");
    for (id, why) in MANUAL {
        let m = &ctx[*id];
        md.push_str(&format!(
            "| {id} | {} | {} | {why} |\n",
            m.title, m.priority
        ));
    }
    let _ = fs::write(cx.out.join("report.md"), md);
    let _ = fs::write(
        cx.out.join("report.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "version": version, "commit": commit, "summary": summary, "scenarios": json,
        }))
        .unwrap_or_default(),
    );
}
