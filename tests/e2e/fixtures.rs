//! Shared, expensive steps: each runs once per suite run and is reused by
//! every scenario that needs it (see [`Ctx::once`]).

use std::fs;

use crate::harness::{blocked, Ctx, Out, Project, Stop, R};

/// Timeout for steps that compile LEZ, the template workspace or guests.
pub const BUILD_TIMEOUT: u64 = 3 * 60 * 60;

fn project(cx: &Ctx, name: &str) -> Project {
    Project {
        root: cx.work.join(name),
    }
}

/// `lgs new <name> <args…>` in the work dir.
///
/// When the work dir is kept between runs (`LGS_E2E_WORKDIR`), the previous
/// project's `target/` is carried over, so dependencies are not recompiled.
pub fn create(cx: &Ctx, name: &str, args: &[&str]) -> (Project, Out) {
    let p = project(cx, name);
    let stash = cx.work.join(".targets").join(name);
    if p.root.join("target").is_dir() {
        let _ = fs::remove_dir_all(&stash);
        let _ = fs::create_dir_all(stash.parent().expect("parent"));
        let _ = fs::rename(p.root.join("target"), &stash);
    }
    let _ = fs::remove_dir_all(&p.root);
    let mut argv = vec!["new", name];
    argv.extend_from_slice(args);
    let out = cx.lgs(&cx.work, &argv).timeout(900).run();
    if out.code == Some(0) && stash.is_dir() {
        let _ = fs::rename(&stash, p.root.join("target"));
    }
    (p, out)
}

/// Build guests with the host toolchain: the suite must not depend on Docker.
pub fn use_local_guest(p: &Project) {
    if !p.toml().contains("\n[build]") {
        p.append_toml("\n[build]\nguest = \"local\"\n");
    }
}

/// The main project (`<work>/main`), created from the default template.
pub fn main_created(cx: &Ctx, owner: bool) -> R<(Project, Out)> {
    let out = cx.once("main: new", owner, || {
        let (p, out) = create(cx, "main", &[]);
        out.ok()?;
        use_local_guest(&p);
        Ok(out)
    })?;
    Ok((project(cx, "main"), out))
}

/// The main project after `lgs setup`.
pub fn main_setup(cx: &Ctx, owner: bool) -> R<(Project, Out)> {
    let (p, _) = main_created(cx, false)?;
    let out = cx.once("main: setup", owner, || {
        let out = cx.lgs(&p.root, &["setup"]).timeout(BUILD_TIMEOUT).run();
        out.ok()?;
        Ok(out)
    })?;
    Ok((p, out))
}

/// The main project after `lgs build` (host-toolchain guests).
pub fn main_built(cx: &Ctx, owner: bool) -> R<(Project, Out)> {
    let (p, _) = main_setup(cx, false)?;
    let out = cx.once("main: build", owner, || {
        let out = cx.lgs(&p.root, &["build"]).timeout(BUILD_TIMEOUT).run();
        out.ok()?;
        Ok(out)
    })?;
    Ok((p, out))
}

/// The main project with its localnet running and a seeded default wallet.
pub fn main_localnet(cx: &Ctx) -> R<Project> {
    let (p, _) = main_setup(cx, false)?;
    cx.localnet_up(&p)?;
    if p.default_address().is_none() {
        // `setup` defers seeding when no sequencer is running (LEZ v0.2.4+).
        let seed = cx.lgs(&p.root, &["setup"]).timeout(BUILD_TIMEOUT).run();
        if seed.code != Some(0) || p.default_address().is_none() {
            blocked!(
                "default wallet was not seeded with localnet up: {}",
                crate::harness::tail(&seed.all(), 800)
            );
        }
    }
    Ok(p)
}

/// The main project after one `wallet topup` of the default account.
pub fn main_topped_up(cx: &Ctx, owner: bool) -> R<(Project, Out)> {
    let p = main_localnet(cx)?;
    let out = cx.once("main: wallet topup", owner, || {
        let out = cx.lgs(&p.root, &["wallet", "topup"]).timeout(600).run();
        out.ok()?;
        Ok(out)
    })?;
    cx.localnet_up(&p)?;
    Ok((p, out))
}

/// The main project with every program deployed to its running localnet.
pub fn main_deployed(cx: &Ctx, owner: bool) -> R<(Project, Out)> {
    let (p, _) = main_built(cx, false)?;
    main_topped_up(cx, false)?;
    let out = cx.once("main: deploy", owner, || {
        let out = cx.lgs(&p.root, &["deploy"]).timeout(1800).run();
        out.ok()?;
        Ok(out)
    })?;
    cx.localnet_up(&p)?;
    Ok((p, out))
}

/// `test-node prepare` for the main project.
pub fn tn_prepared(cx: &Ctx, owner: bool) -> R<(Project, Out)> {
    let (p, _) = main_created(cx, false)?;
    let root = p.root.to_string_lossy().into_owned();
    let out = cx.once("main: test-node prepare", owner, || {
        let out = cx
            .lgs(
                &cx.work,
                &["-q", "test-node", "prepare", "--project", &root, "--json"],
            )
            .timeout(BUILD_TIMEOUT)
            .run();
        out.ok()?;
        Ok(out)
    })?;
    Ok((p, out))
}

/// A second, fresh project taken to deployed programs by a bare `lgs run`.
pub fn run_project(cx: &Ctx, owner: bool) -> R<(Project, Out)> {
    let out = cx.once("run: new + run", owner, || {
        let (p, created) = create(cx, "run", &[]);
        if created.code != Some(0) {
            return Err(Stop::Blocked(format!(
                "could not create the run project: {}",
                crate::harness::tail(&created.all(), 600)
            )));
        }
        use_local_guest(&p);
        cx.localnet_down();
        let out = cx.lgs(&p.root, &["run"]).timeout(BUILD_TIMEOUT).run();
        if out.code == Some(0) {
            cx.localnet_mark(&p, true);
        }
        out.ok()?;
        Ok(out)
    })?;
    let p = project(cx, "run");
    cx.localnet_up(&p)?;
    Ok((p, out))
}

/// A project from the spel template (`<work>/spel`).
pub fn spel_created(cx: &Ctx, owner: bool) -> R<(Project, Out)> {
    let out = cx.once("spel: new", owner, || {
        let p = project(cx, "spel");
        let _ = fs::remove_dir_all(&p.root);
        let out = cx
            .lgs(&cx.work, &["new", "spel", "--template", "spel"])
            .timeout(BUILD_TIMEOUT)
            .run();
        out.ok()?;
        Ok(out)
    })?;
    Ok((project(cx, "spel"), out))
}
