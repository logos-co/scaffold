//! `.dogfooding/features/project-creation.md`

use std::fs;

use crate::fixtures;
use crate::harness::{check, tail, Ctx, Project, Restore, Stop, R};
use crate::scenarios::{files_under, ls, toml_section, toml_value};

/// An LEZ commit older than the default pin (the v0.2.3 release line).
const OLDER_LEZ_PIN: &str = "43b66b15928ddd30a1ed71a176139c98ac6c2ca6";

/// CREATE-001 — Create a default-template project
pub fn default_project(cx: &Ctx) -> R {
    let (p, out) = fixtures::main_created(cx, true)?;
    out.has(&p.root.to_string_lossy())?
        .has("Pinned lez:")?
        .has("Cache root:")?;
    let toml = p.toml();
    for header in [
        "[scaffold]",
        "[repos.lez]",
        "[repos.spel]",
        "[repos.basecamp]",
        "[repos.lgpm]",
        "[circuits]",
        "[wallet]",
    ] {
        check!(
            toml.lines().any(|l| l.trim() == header),
            "scaffold.toml has no {header}"
        );
    }
    check!(
        toml_value(&toml_section(&toml, "[scaffold]"), "version").as_deref() == Some("0.2.0"),
        "scaffold.toml is not schema 0.2.0"
    );
    for repo in ["[repos.lez]", "[repos.spel]"] {
        let section = toml_section(&toml, repo);
        check!(toml_value(&section, "pin").is_some(), "{repo} has no pin");
        check!(
            toml_value(&section, "path").is_none(),
            "{repo} has a `path` in a non-vendored project"
        );
    }
    check!(
        toml_value(&toml_section(&toml, "[wallet]"), "home_dir").is_some(),
        "[wallet] has no home_dir"
    );
    check!(
        !toml.lines().any(|l| l.trim_start().starts_with("binary")),
        "scaffold.toml has a wallet `binary` field"
    );
    let runners = ls(&p.path("src/bin"));
    check!(
        runners.iter().any(|f| f == "run_hello_world.rs")
            && runners
                .iter()
                .filter(|f| f.starts_with("run_hello_world"))
                .count()
                > 1,
        "src/bin has no run_hello_world*.rs runners: {runners:?}"
    );
    let cargo = p.read("Cargo.toml");
    for dep in ["risc0-zkvm", "risc0-build"] {
        let line = cargo
            .lines()
            .find(|l| l.trim_start().starts_with(dep))
            .unwrap_or("");
        check!(
            line.contains("\"="),
            "Cargo.toml does not pin {dep} with an exact `=` requirement: {line:?}"
        );
    }
    Ok(())
}

/// CREATE-003 — Template selection and the create alias
pub fn templates_and_alias(cx: &Ctx) -> R {
    // Step 1: `create` produces the same project as `new`.
    let (by_new, out) = fixtures::create(cx, "tmpl-new", &[]);
    out.ok()?;
    let by_create = Project {
        root: cx.work.join("tmpl-create"),
    };
    let _ = fs::remove_dir_all(&by_create.root);
    cx.lgs(&cx.work, &["create", "tmpl-create"])
        .timeout(900)
        .run()
        .ok()?;
    let a = files_under(&by_new.root, &[".scaffold"]);
    let b = files_under(&by_create.root, &[".scaffold"]);
    check!(
        a == b && !a.is_empty(),
        "`create` and `new` produced different file sets: only new {:?}, only create {:?}",
        a.difference(&b).collect::<Vec<_>>(),
        b.difference(&a).collect::<Vec<_>>()
    );

    // Step 2: an unknown template is an error, not a silent default.
    let bad = cx
        .lgs(
            &cx.work,
            &["new", "tmpl-invalid", "--template", "nonexistent-template"],
        )
        .run();
    bad.fails()?
        .has("`default`")?
        .has("`spel`")?
        .has("lez-framework")?;
    check!(
        !cx.work.join("tmpl-invalid").exists(),
        "a rejected template still created a directory"
    );

    // Step 3: the deprecated alias still makes a spel project. It needs the
    // spel CLI, which SPEL-001 reports on.
    match fixtures::spel_created(cx, false) {
        Ok(_) => {
            let _ = fs::remove_dir_all(cx.work.join("tmpl-alias"));
            let alias = cx
                .lgs(
                    &cx.work,
                    &["new", "tmpl-alias", "--template", "lez-framework"],
                )
                .timeout(900)
                .run();
            alias.ok()?.has("deprecated")?.has("--template spel")?;
            check!(
                cx.work.join("tmpl-alias/spel.toml").is_file(),
                "`--template lez-framework` did not produce a spel project"
            );
        }
        Err(Stop::Blocked(m)) | Err(Stop::Skip(m)) | Err(Stop::Fail(m)) => {
            cx.note(format!(
                "step 3 (lez-framework alias) not run: the spel CLI is unavailable ({})",
                tail(m.lines().next().unwrap_or(""), 120)
            ));
        }
    }
    Ok(())
}

fn git_head(cx: &Ctx, dir: &std::path::Path) -> String {
    cx.cmd(dir, "git", &["rev-parse", "HEAD"])
        .run()
        .stdout
        .trim()
        .to_string()
}

/// CREATE-004 — Vendor dependencies into the project
pub fn vendor_deps(cx: &Ctx) -> R {
    let (p, out) = fixtures::create(cx, "vendor", &["--vendor-deps"]);
    out.ok()?;
    let lez = toml_section(&p.toml(), "[repos.lez]");
    check!(
        toml_value(&lez, "path").as_deref() == Some(".scaffold/repos/lez"),
        "[repos.lez].path is not the relative vendored path: {lez}"
    );
    let head = git_head(cx, &p.path(".scaffold/repos/lez"));
    check!(
        Some(head.as_str()) == toml_value(&lez, "pin").as_deref(),
        "vendored LEZ HEAD {head} is not the recorded pin"
    );

    if fixtures::spel_created(cx, false).is_err() {
        cx.note("step 2 (vendored spel project) not run: the spel CLI is unavailable");
        return Ok(());
    }
    let sp = Project {
        root: cx.work.join("vendor-spel"),
    };
    let _ = fs::remove_dir_all(&sp.root);
    let out = cx
        .lgs(
            &cx.work,
            &["new", "vendor-spel", "--template", "spel", "--vendor-deps"],
        )
        .timeout(fixtures::BUILD_TIMEOUT)
        .run();
    out.ok()?.lacks("already exists")?;
    let spel = toml_section(&sp.toml(), "[repos.spel]");
    check!(
        toml_value(&spel, "path").as_deref() == Some(".scaffold/repos/spel"),
        "[repos.spel].path is not the relative vendored path: {spel}"
    );
    let head = git_head(cx, &sp.path(".scaffold/repos/spel"));
    check!(
        Some(head.as_str()) == toml_value(&spel, "pin").as_deref(),
        "vendored spel HEAD {head} is not the recorded pin"
    );
    cx.discard(&["vendor", "vendor-spel"]);
    Ok(())
}

/// CREATE-005 — Cache checkouts are isolated by cache root, pin and LEZ source
pub fn cache_isolation(cx: &Ctx) -> R {
    const ENV: &str = "LOGOS_SCAFFOLD_CACHE_ROOT";
    let custom = cx.empty_dir("custom-cache");
    let custom_s = custom.to_string_lossy().into_owned();
    let lez_root = custom.join("repos/lez");
    let new = |name: &str, extra: &[&str]| {
        let _ = fs::remove_dir_all(cx.work.join(name));
        let mut args = vec!["new", name, "--cache-root", custom_s.as_str()];
        args.extend_from_slice(extra);
        cx.lgs(&cx.work, &args).env_remove(ENV).timeout(900).run()
    };

    // Step 1: the flag is honoured and recorded.
    new("cache-a", &[]).ok()?.has(&custom_s)?;
    let a = Project {
        root: cx.work.join("cache-a"),
    };
    check!(
        toml_value(&toml_section(&a.toml(), "[scaffold]"), "cache_root").as_deref()
            == Some(custom_s.as_str()),
        "scaffold.toml does not record cache_root = {custom_s}"
    );
    let pin = toml_value(&toml_section(&a.toml(), "[repos.lez]"), "pin").unwrap_or_default();
    let first = lez_root.join(&pin);
    check!(
        first.join(".git").exists(),
        "no checkout at {}",
        first.display()
    );
    let first_head = git_head(cx, &first);

    // Step 2: another pin gets its own checkout. Only the clone matters.
    new("cache-b", &[]).ok()?;
    let b = Project {
        root: cx.work.join("cache-b"),
    };
    fs::write(
        b.path("scaffold.toml"),
        b.toml().replace(&pin, OLDER_LEZ_PIN),
    )
    .expect("write scaffold.toml");
    let second = lez_root.join(OLDER_LEZ_PIN);
    // Without `cargo`, `setup` checks the pin out and stops at its first build.
    let setup = cx
        .lgs(&b.root, &["setup"])
        .env_remove(ENV)
        .env("PATH", crate::harness::path_without("cargo"))
        .timeout(900)
        .run();
    check!(
        git_head(cx, &second) == OLDER_LEZ_PIN,
        "`setup` did not check the older pin out into {}:\n{}",
        second.display(),
        tail(&setup.all(), 1200)
    );

    // Step 3: a non-default LEZ source gets `<pin>-src-<hash>`.
    let src = cx.work.join("lez-src");
    let _ = fs::remove_dir_all(&src);
    cx.cmd(
        &cx.work,
        "git",
        &[
            "clone",
            "-q",
            &first.to_string_lossy(),
            &src.to_string_lossy(),
        ],
    )
    .timeout(600)
    .run()
    .ok()?;
    new("cache-src", &["--lez-path", &src.to_string_lossy()]).ok()?;
    let pins = cx
        .lgs(
            &cx.work,
            &[
                "-q",
                "test-node",
                "pins",
                "--project",
                "cache-src",
                "--json",
            ],
        )
        .env_remove(ENV)
        .run()
        .json()?;
    let checkout = pins["lez_checkout"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    check!(
        checkout.starts_with(&format!("{}-src-", first.display()))
            && pins["checkout_ownership"] == "managed_cache",
        "a non-default LEZ source did not get a managed `<pin>-src-<hash>` checkout: {pins}"
    );
    check!(
        std::path::Path::new(&checkout).join(".git").exists(),
        "{checkout} was not cloned"
    );

    // Step 4: three checkouts, each at its own commit; the first never moved.
    let dirs = ls(&lez_root);
    check!(
        dirs.len() == 3,
        "expected three checkouts under {}: {dirs:?}",
        lez_root.display()
    );
    check!(
        git_head(cx, &first) == first_head,
        "the first checkout's HEAD moved"
    );
    cx.lgs(&a.root, &["doctor"])
        .env_remove(ENV)
        .timeout(300)
        .run()
        .has("(from scaffold.toml [scaffold].cache_root)")?;

    // Step 5: a source that does not exist is named in the error.
    let bad = new("cache-bad", &["--lez-path", "/does/not/exist"]);
    bad.fails()?.has("/does/not/exist")?;
    cx.discard(&["custom-cache", "cache-a", "cache-b", "cache-src", "lez-src"]);
    Ok(())
}

/// CREATE-007 — Adopt an existing directory with init
pub fn init_adopts(cx: &Ctx) -> R {
    let dir = cx.empty_dir("adopt");
    cx.lgs(&dir, &["init", "--dry-run"]).run().ok()?;
    check!(
        ls(&dir).is_empty(),
        "`init --dry-run` wrote files: {:?}",
        ls(&dir)
    );
    cx.lgs(&dir, &["init"]).run().ok()?;
    for rel in [
        "scaffold.toml",
        ".scaffold/state",
        ".scaffold/logs",
        "AGENTS.md",
        ".claude/skills",
        ".cursor/rules",
    ] {
        check!(dir.join(rel).exists(), "`init` did not create {rel}");
    }
    let toml = fs::read_to_string(dir.join("scaffold.toml")).unwrap_or_default();
    check!(
        toml_value(&toml_section(&toml, "[scaffold]"), "version").as_deref() == Some("0.2.0"),
        "`init` did not write a schema-0.2.0 scaffold.toml"
    );
    let ignore = fs::read_to_string(dir.join(".gitignore")).unwrap_or_default();
    check!(
        ignore.lines().any(|l| l.trim() == ".scaffold"),
        ".gitignore does not list .scaffold"
    );
    Ok(())
}

/// CREATE-008 — Migrate an old scaffold.toml with init
pub fn init_migrates(cx: &Ctx) -> R {
    let (main, _) = fixtures::main_created(cx, false)?;
    let dir = cx.empty_dir("migrate");
    let toml_path = dir.join("scaffold.toml");
    // Step 1: an old-schema file with a legacy url, a comment and the old
    // basecamp tables.
    let old = main
        .toml()
        .replacen("version = \"0.2.0\"", "version = \"0.1.0\"", 1)
        .replacen(
            "[repos.lez]\n",
            "# e2e: this comment must survive\n[repos.lez]\nurl = \"https://example\"\n",
            1,
        )
        + "\n[basecamp]\npin = \"abc123\"\nsource = \"https://github.com/logos-co/logos-basecamp.git\"\nlgpm_flake = \"github:logos-co/logos-package-manager\"\n\n[basecamp.modules.my_mod]\nflake = \"path:./mod\"\nrole = \"project\"\n";
    check!(
        old.contains("0.1.0") && old.contains("url ="),
        "could not build the old-schema fixture"
    );
    fs::write(&toml_path, &old).expect("write old scaffold.toml");
    let _restore = Restore::of(&toml_path);

    // Step 2: everything but `init` refuses, in one line, without JSON.
    let doctor = cx.lgs(&dir, &["doctor", "--json"]).run();
    doctor.fails()?.has("logos-scaffold init")?;
    check!(
        !doctor.stdout.trim_start().starts_with('{'),
        "doctor printed JSON for an old-schema project"
    );

    // Step 3: dry run writes nothing; init migrates in place.
    cx.lgs(&dir, &["init", "--dry-run"]).run().ok()?;
    check!(
        fs::read_to_string(&toml_path).unwrap_or_default() == old,
        "`init --dry-run` changed scaffold.toml"
    );
    let init = cx.lgs(&dir, &["init"]).run();
    init.ok()?;
    let new = fs::read_to_string(&toml_path).unwrap_or_default();
    check!(
        dir.join("scaffold.toml.bak").is_file(),
        "`init` wrote no scaffold.toml.bak"
    );
    check!(
        toml_value(&toml_section(&new, "[scaffold]"), "version").as_deref() == Some("0.2.0"),
        "version was not bumped to 0.2.0"
    );
    check!(
        !new.contains("url ="),
        "the legacy `url` survived the migration"
    );
    check!(
        new.contains("# e2e: this comment must survive"),
        "the comment was lost"
    );
    check!(
        toml_value(&toml_section(&new, "[repos.basecamp]"), "pin").as_deref() == Some("abc123"),
        "[repos.basecamp].pin was not taken from the old [basecamp] table"
    );
    check!(
        toml_value(&toml_section(&new, "[repos.lgpm]"), "pin").is_some(),
        "[repos.lgpm] has no pin"
    );
    check!(
        new.contains("[modules.my_mod]"),
        "[basecamp.modules.my_mod] was not moved to [modules.my_mod]"
    );
    check!(
        !new.lines()
            .any(|l| l.trim() == "[basecamp]" || l.starts_with("[basecamp.modules")),
        "old [basecamp] tables are still there"
    );
    // The old lgpm_flake had no /<rev>, so the default pin is kept with a warning.
    init.has("!")?;

    // Step 4: a second init changes nothing.
    cx.lgs(&dir, &["init"]).run().ok()?;
    check!(
        fs::read_to_string(&toml_path).unwrap_or_default() == new,
        "a second `init` changed an already-current scaffold.toml"
    );
    Ok(())
}
