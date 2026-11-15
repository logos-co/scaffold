//! `.dogfooding/features/setup-and-build.md`

use crate::fixtures;
use crate::harness::{check, Ctx, R};

/// BUILD-003 — Missing Docker toolchain fails fast with an escape hatch
pub fn missing_docker_toolchain(cx: &Ctx) -> R {
    // A project in the default (docker) guest mode; nothing is ever built in it.
    let (p, out) = fixtures::create(cx, "nodocker", &[]);
    out.ok()?;

    if cx.caps.docker {
        cx.note("steps 1-2 (daemon down) not run: a Docker daemon is running on this host");
    } else {
        let build = cx.lgs(&p.root, &["build"]).run();
        build
            .fails()?
            .within(60.0)?
            .has("guest = \"local\"")?
            .lacks("Compiling")?;
        check!(
            build.all().contains("docker") || build.all().contains("cargo-risczero"),
            "the refusal does not name the missing piece: {}",
            build.all()
        );
        let doctor = cx.lgs(&p.root, &["doctor"]).timeout(300).run();
        let row = doctor
            .lines_with("| guest build |")
            .first()
            .cloned()
            .unwrap_or_default();
        check!(
            row.starts_with("FAIL") && row.contains("docker (risczero/risc0-guest-builder:"),
            "doctor's `guest build` row does not agree with the refusing build: {row:?}"
        );
    }

    // Step 3: no cargo-risczero on PATH.
    let path = std::env::var("PATH").unwrap_or_default();
    let stripped: Vec<&str> = path
        .split(':')
        .filter(|d| !d.is_empty() && !std::path::Path::new(d).join("cargo-risczero").exists())
        .collect();
    let build = cx
        .lgs(&p.root, &["build"])
        .env("PATH", stripped.join(":"))
        .run();
    build
        .fails()?
        .within(60.0)?
        .has("guest = \"local\"")?
        .lacks("Compiling")?;
    if cx.caps.docker {
        build
            .has("cargo-risczero")?
            .has("rzup install cargo-risczero")?;
    }
    Ok(())
}

/// Names of the guest programs of a default-template project.
pub fn programs(p: &crate::harness::Project) -> Vec<String> {
    crate::scenarios::ls(&p.path("methods/guest/src/bin"))
        .into_iter()
        .filter_map(|f| f.strip_suffix(".rs").map(str::to_string))
        .collect()
}

/// `.bin` files under `dir`, found recursively, whose path contains `marker`.
pub fn guest_bins(dir: &std::path::Path, marker: &str) -> Vec<std::path::PathBuf> {
    crate::scenarios::files_under(dir, &[])
        .into_iter()
        .filter(|f| f.ends_with(".bin") && f.contains(marker))
        .map(|f| dir.join(f))
        .collect()
}

/// BUILD-001 — setup builds the project-local toolchain
pub fn setup_builds_toolchain(cx: &Ctx) -> R {
    let (p, out) = fixtures::main_setup(cx, true)?;
    out.has("setup complete")?
        .lacks("could not seed default wallet automatically")?;
    check!(
        out.all().contains("default wallet seeded")
            || out.all().contains("default wallet seeding deferred"),
        "setup printed none of the default-wallet seeding lines"
    );
    let root = p.root.to_string_lossy().into_owned();
    let pins = cx
        .lgs(
            &cx.work,
            &["-q", "test-node", "pins", "--project", &root, "--json"],
        )
        .run()
        .json()?;
    let lez = std::path::PathBuf::from(pins["lez_checkout"].as_str().unwrap_or_default());
    for bin in ["sequencer_service", "wallet"] {
        check!(
            lez.join("target/release").join(bin).is_file(),
            "setup did not build {bin} in {}",
            lez.display()
        );
    }
    check!(
        p.path(".scaffold/circuits/pol/verification_key.json")
            .is_file(),
        "setup did not materialize the circuits into .scaffold/circuits"
    );
    // Nothing is installed to PATH.
    for tool in ["wallet", "spel", "sequencer_service"] {
        let found = cx.sh(&p.root, &format!("command -v {tool}")).run();
        check!(
            !found
                .stdout
                .trim()
                .starts_with(&*cx.cache.to_string_lossy()),
            "`{tool}` from the scaffold cache is on PATH: {}",
            found.stdout.trim()
        );
    }
    Ok(())
}

/// BUILD-002 — Default build produces deterministic guest binaries
pub fn docker_build(cx: &Ctx) -> R {
    if !(cx.caps.docker && cx.caps.cargo_risczero) {
        crate::harness::skip!("needs a running Docker daemon and cargo-risczero on PATH");
    }
    fixtures::main_setup(cx, false)?;
    let (p, out) = fixtures::create(cx, "docker", &[]);
    out.ok()?;
    let build = cx
        .lgs(&p.root, &["build"])
        .timeout(fixtures::BUILD_TIMEOUT)
        .run();
    build
        .ok()?
        .has("Building guest methods (deterministic, risc0-guest-builder:")?
        .has("ImageID:")?;
    let bins = guest_bins(&p.path("target/riscv-guest-docker"), "/docker/");
    for program in programs(&p) {
        check!(
            bins.iter().any(|b| b.ends_with(format!("{program}.bin"))),
            "the docker build left no {program}.bin under target/riscv-guest-docker: {bins:?}"
        );
    }
    // The `[project-path]` form, from outside the project.
    cx.lgs(&cx.work, &["build", &p.root.to_string_lossy()])
        .timeout(fixtures::BUILD_TIMEOUT)
        .run()
        .ok()?
        .lacks("must be run inside a logos-scaffold project")?;
    Ok(())
}

/// BUILD-005 — Local guest build opt-out
pub fn local_guest_build(cx: &Ctx) -> R {
    let (p, first) = fixtures::main_built(cx, true)?;
    first.lacks("risc0-guest-builder")?;
    // A leftover deterministic tree is cleared by a local build.
    std::fs::create_dir_all(p.path("target/riscv-guest-docker/stale")).expect("mkdir");
    let build = cx
        .lgs(&p.root, &["build", "--guest", "local"])
        .timeout(fixtures::BUILD_TIMEOUT)
        .run();
    build
        .ok()?
        .has("Clearing deterministic guest artefacts in")?;
    check!(
        !p.path("target/riscv-guest-docker").exists(),
        "`build --guest local` left target/riscv-guest-docker in place"
    );
    let bins = guest_bins(&p.path("target/riscv-guest"), "/release/");
    for program in programs(&p) {
        check!(
            bins.iter().any(|b| b.ends_with(format!("{program}.bin"))),
            "the local build left no release {program}.bin: {bins:?}"
        );
    }
    let doctor = cx.lgs(&p.root, &["doctor"]).timeout(300).run();
    let row = |name: &str| {
        doctor
            .lines_with(&format!("| {name} |"))
            .first()
            .cloned()
            .unwrap_or_default()
    };
    let guest = row("guest build");
    check!(
        guest.starts_with("PASS") && guest.contains("local") && guest.contains("not reproducible"),
        "doctor's `guest build` row for a local-guest project: {guest:?}"
    );
    let runtime = row("container runtime");
    check!(
        !runtime.contains("guest builds will fail"),
        "doctor warns that guest builds will fail in a local-guest project: {runtime:?}"
    );
    Ok(())
}

/// BUILD-006 — setup --prebuilt installs only a verified sequencer
pub fn prebuilt_sequencer(cx: &Ctx) -> R {
    // A cache root where the pin's sequencer is not built, so the prebuilt
    // path is taken.
    let cache = cx.empty_dir("prebuilt-cache");
    let cache_s = cache.to_string_lossy().into_owned();
    let _ = std::fs::remove_dir_all(cx.work.join("prebuilt"));
    cx.lgs(&cx.work, &["new", "prebuilt", "--cache-root", &cache_s])
        .env_remove("LOGOS_SCAFFOLD_CACHE_ROOT")
        .timeout(900)
        .run()
        .ok()?;
    let root = cx.work.join("prebuilt");
    let mut setup = cx
        .lgs(&root, &["setup", "--prebuilt"])
        .env_remove("LOGOS_SCAFFOLD_CACHE_ROOT")
        .spawn(&cx.out.join("logs/BUILD-006-setup.log"))?;
    const FALLBACK: &str = "falling back to source build";
    const UNVERIFIED: &str = "not installing an unverified binary";
    let outcome = cx.poll(600, 500, || {
        let log = setup.log_text();
        if log.contains(FALLBACK) || log.contains(UNVERIFIED) {
            Some("source")
        } else if !setup.running() {
            Some("finished")
        } else {
            None
        }
    });
    let log = setup.log_text();
    check!(
        log.contains("Checking for prebuilt binaries (tag: "),
        "`setup --prebuilt` never checked for a prebuilt:\n{}",
        crate::harness::tail(&log, 1200)
    );
    let installed = crate::scenarios::files_under(&cache.join("repos/lez"), &[])
        .iter()
        .any(|f| f.ends_with("target/release/sequencer_service"));
    match outcome {
        Some("source") => {
            // Stop here: the source build is BUILD-001's subject.
            check!(
                !installed,
                "a sequencer binary was installed before the source build finished"
            );
            cx.note("no verified prebuilt for this pin: fallback to the source build seen, verification path not exercised");
        }
        Some(_) => {
            check!(
                setup.wait(1) == Some(0) && installed,
                "`setup --prebuilt` ended without a fallback line or an installed sequencer:\n{}",
                crate::harness::tail(&log, 1200)
            );
        }
        None => crate::harness::fail!(
            "`setup --prebuilt` printed no outcome within 10 minutes:\n{}",
            crate::harness::tail(&log, 1200)
        ),
    }
    Ok(())
}
