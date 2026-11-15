//! `.dogfooding/features/cli-basics.md`

use crate::fixtures;
use crate::harness::{check, Ctx, R};
use crate::scenarios::ls;

const TOP_LEVEL: &[&str] = &[
    "create",
    "new",
    "init",
    "setup",
    "build",
    "deploy",
    "localnet",
    "test-node",
    "wallet",
    "spel",
    "basecamp",
    "doctor",
    "run",
    "report",
    "tui",
    "completions",
];
/// Commands whose own `--help` has no `Examples:` block (their subcommands do).
const NO_EXAMPLES: &[&str] = &["localnet", "test-node", "basecamp", "run"];

/// CLI-001 — Help, version and unknown commands
pub fn help_version_unknown(cx: &Ctx) -> R {
    let dir = cx.empty_dir("cli-001");
    let lgs = |args: &[&str]| cx.lgs(&dir, args).run();

    // Step 1: one version, under both names, matching Cargo.toml.
    let version = format!("logos-scaffold {}", env!("CARGO_PKG_VERSION"));
    let v = lgs(&["--version"]);
    v.ok()?;
    check!(
        v.stdout.trim() == version,
        "--version printed {:?}, expected {version:?}",
        v.stdout
    );
    let alias = cx
        .cmd(&dir, &cx.lgs_alias.to_string_lossy(), &["--version"])
        .run();
    alias.ok()?;
    check!(
        alias.stdout.trim().ends_with(env!("CARGO_PKG_VERSION")),
        "`lgs --version` printed {:?}",
        alias.stdout
    );

    // Step 2: `help`, `--help` and a bare invocation agree; `help <cmd>` is `<cmd> --help`.
    let long = lgs(&["--help"]);
    long.ok()?;
    let help = lgs(&["help"]);
    check!(
        help.code == Some(0) && help.stdout == long.stdout,
        "`help` differs from `--help`"
    );
    let bare = lgs(&[]);
    check!(
        bare.code == Some(0) && bare.stdout == long.stdout,
        "a bare invocation differs from `--help`"
    );
    let a = lgs(&["help", "deploy"]);
    let b = lgs(&["deploy", "--help"]);
    check!(
        a.code == Some(0) && a.stdout == b.stdout,
        "`help deploy` differs from `deploy --help`"
    );
    lgs(&["help", "test-node", "start"])
        .ok()?
        .has("Usage: logos-scaffold test-node start")?;

    // Step 3: every top-level command answers --help and touches nothing.
    for c in TOP_LEVEL {
        let o = lgs(&[c, "--help"]);
        o.ok()?;
        let has_examples = o.stdout.lines().any(|l| l.starts_with("Examples"));
        check!(
            has_examples != NO_EXAMPLES.contains(c),
            "`{c} --help` {} an Examples block; the scenario expects the opposite",
            if has_examples { "has" } else { "lacks" }
        );
    }
    check!(
        ls(&dir).is_empty(),
        "--help left files behind: {:?}",
        ls(&dir)
    );

    // Step 4: unknown commands.
    for args in [&["nonexistent-command"][..], &["help", "nope"][..]] {
        lgs(args)
            .fails()?
            .has("unrecognized subcommand")?
            .has("--help")?;
    }
    Ok(())
}

/// CLI-003 — Project commands outside a project fail clearly
pub fn outside_project(cx: &Ctx) -> R {
    let dir = cx.empty_dir("cli-003");
    for args in [
        &["build"][..],
        &["deploy"],
        &["doctor"],
        &["localnet", "status"],
        &["wallet", "list"],
        &["basecamp", "doctor"],
    ] {
        let o = cx.lgs(&dir, args).run();
        o.fails()?
            .has("This command must be run inside a logos-scaffold project")?
            .has("logos-scaffold create <name>")?
            .has("Searched from")?
            .lacks("No such file or directory")?;
    }
    // The next step follows argv[0].
    cx.cmd(&dir, &cx.lgs_alias.to_string_lossy(), &["doctor"])
        .run()
        .fails()?
        .has("lgs create <name>")?;
    Ok(())
}

fn echoes(stdout: &str) -> usize {
    stdout.lines().filter(|l| l.starts_with("$ ")).count()
}

/// CLI-006 — Quiet mode suppresses command echoes
pub fn quiet_mode(cx: &Ctx) -> R {
    let (p, _) = fixtures::main_setup(cx, false)?;
    let setup = |args: &[&str], quiet_env: bool| {
        let mut c = cx.lgs(&p.root, args).timeout(fixtures::BUILD_TIMEOUT);
        if quiet_env {
            c = c.env("LOGOS_SCAFFOLD_QUIET", "1");
        }
        c.run()
    };
    let loud = setup(&["setup"], false);
    loud.ok()?;
    check!(
        echoes(&loud.stdout) > 0,
        "plain `setup` echoed no `$ …` command lines"
    );
    let quiet = setup(&["-q", "setup"], false);
    quiet.ok()?.has("setup complete")?;
    check!(
        echoes(&quiet.stdout) == 0,
        "`-q setup` still echoed commands:\n{}",
        quiet.stdout
    );
    let env = setup(&["setup"], true);
    env.ok()?.has("setup complete")?;
    check!(
        echoes(&env.stdout) == 0,
        "LOGOS_SCAFFOLD_QUIET=1 still echoed commands"
    );
    Ok(())
}

/// CLI-008 — A closed output pipe ends the command quietly
pub fn closed_pipe(cx: &Ctx) -> R {
    let dir = cx.empty_dir("cli-008");
    let piped = |cwd: &std::path::Path, args: &str| -> R {
        let o = cx
            .sh(
                cwd,
                &format!("\"$LGS\" {args} 2>err.txt | head -1 >/dev/null; echo \"status=${{PIPESTATUS[0]}}\"; cat err.txt; rm -f err.txt"),
            )
            .run();
        o.ok()?;
        let status = o.stdout.lines().next().unwrap_or("");
        check!(
            status == "status=141" || status == "status=0",
            "`{args} | head -1` ended with {status} (expected 141, or 0 for short output)"
        );
        check!(
            o.stdout.lines().count() == 1,
            "`{args} | head -1` wrote to stderr:\n{}",
            o.stdout
        );
        Ok(())
    };
    for args in ["--help", "completions zsh", "test-node start --help"] {
        piped(&dir, args)?;
    }
    let (p, _) = fixtures::main_setup(cx, false)?;
    piped(&p.root, "doctor")
}
