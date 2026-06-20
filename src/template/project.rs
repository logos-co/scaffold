use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail};
use include_dir::{include_dir, Dir};

use crate::state::write_text;
use crate::DynResult;

static TEMPLATES_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/templates");

pub(crate) struct OverlayRenderContext<'a> {
    pub(crate) crate_name: &'a str,
    pub(crate) lez_pin: &'a str,
}

pub(crate) fn apply_overlay(
    target: &Path,
    variant: &str,
    ctx: &OverlayRenderContext<'_>,
) -> DynResult<()> {
    apply_overlay_variant(target, variant, ctx)?;
    ensure_scaffold_in_gitignore(target)
}

pub(crate) fn ensure_scaffold_in_gitignore(target: &Path) -> DynResult<()> {
    let gitignore_path = target.join(".gitignore");
    let mut content = if gitignore_path.exists() {
        fs::read_to_string(&gitignore_path)?
    } else {
        String::new()
    };

    let already_present = content.lines().any(|l| l.trim() == ".scaffold");
    if !already_present {
        if !content.ends_with('\n') && !content.is_empty() {
            content.push('\n');
        }
        content.push_str(".scaffold\n");
        write_text(&gitignore_path, &content)?;
    }
    Ok(())
}

fn apply_overlay_variant(
    target: &Path,
    variant: &str,
    ctx: &OverlayRenderContext<'_>,
) -> DynResult<()> {
    let variant_dir = TEMPLATES_DIR
        .get_dir(variant)
        .ok_or_else(|| anyhow!("template variant not found: {variant}"))?;

    apply_dir_recursive(variant_dir, target, &PathBuf::new(), ctx)
}

fn apply_dir_recursive(
    dir: &Dir<'_>,
    target_root: &Path,
    relative: &Path,
    ctx: &OverlayRenderContext<'_>,
) -> DynResult<()> {
    for file in dir.files() {
        let file_name = file
            .path()
            .file_name()
            .ok_or_else(|| anyhow!("invalid template file path: {}", file.path().display()))?;

        let output_file_name = normalize_template_file_name(file_name);
        let rel_path = relative.join(output_file_name);
        let output_path = target_root.join(&rel_path);

        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let raw = file
            .contents_utf8()
            .ok_or_else(|| anyhow!("template is not valid UTF-8: {}", file.path().display()))?;
        let rendered = render_template_text(raw, ctx)?;

        write_text(&output_path, &rendered)?;
    }

    for child in dir.dirs() {
        let dir_name = child
            .path()
            .file_name()
            .ok_or_else(|| anyhow!("invalid template dir path: {}", child.path().display()))?;
        let child_relative = relative.join(dir_name);
        apply_dir_recursive(child, target_root, &child_relative, ctx)?;
    }

    Ok(())
}

fn normalize_template_file_name(file_name: &std::ffi::OsStr) -> std::ffi::OsString {
    if file_name == std::ffi::OsStr::new("Cargo.toml.template") {
        std::ffi::OsString::from("Cargo.toml")
    } else {
        file_name.to_os_string()
    }
}

fn render_template_text(raw: &str, ctx: &OverlayRenderContext<'_>) -> DynResult<String> {
    let rendered = raw
        .replace("{{crate_name}}", ctx.crate_name)
        .replace("{{lez_pin}}", ctx.lez_pin);

    if let Some(token) = find_unresolved_placeholder(&rendered) {
        bail!("unresolved template token `{token}`");
    }

    Ok(rendered)
}

fn find_unresolved_placeholder(text: &str) -> Option<&str> {
    let start = text.find("{{")?;
    let after_open = &text[start + 2..];

    if let Some(end_rel) = after_open.find("}}") {
        Some(&text[start..start + 2 + end_rel + 2])
    } else {
        Some(&text[start..])
    }
}

pub(crate) fn available_templates() -> Vec<String> {
    let mut names: Vec<String> = TEMPLATES_DIR
        .dirs()
        .map(|d| {
            d.path()
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string()
        })
        .filter(|s| !s.is_empty())
        .collect();
    names.sort();
    names
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::str::FromStr;

    use super::{apply_overlay, render_template_text, OverlayRenderContext};
    use tempfile::{tempdir, TempDir};

    fn mk_temp_dir() -> TempDir {
        tempdir().expect("failed to create temporary test directory")
    }

    #[test]
    fn overlay_writes_expected_files() {
        let temp = mk_temp_dir();
        let target = temp.path();
        let ctx = OverlayRenderContext {
            crate_name: "my-app",
            lez_pin: "abc123",
        };

        apply_overlay(target, "default", &ctx).expect("failed to apply default overlay");

        let expected = [
            "Cargo.toml",
            "README.md",
            ".gitignore",
            ".env.local",
            ".scaffold/commands.md",
            "src/lib.rs",
            "src/bin/run_hello_world.rs",
            "src/bin/run_hello_world_private.rs",
            "src/bin/run_hello_world_with_authorization.rs",
            "src/bin/run_hello_world_through_tail_call.rs",
            "src/bin/run_hello_world_through_tail_call_private.rs",
            "src/bin/run_hello_world_with_authorization_through_tail_call_with_pda.rs",
            "src/bin/run_hello_world_with_move_function.rs",
        ];

        for path in expected {
            assert!(target.join(path).exists(), "missing expected file: {path}");
        }
        assert!(
            !target.join("Cargo.toml.template").exists(),
            "template Cargo.toml placeholder should not leak into output"
        );
    }

    #[test]
    fn lez_framework_overlay_converts_template_manifests_to_cargo_toml() {
        let temp = mk_temp_dir();
        let target = temp.path();
        let ctx = OverlayRenderContext {
            crate_name: "my-app",
            lez_pin: "abc123",
        };

        apply_overlay(target, "lez-framework", &ctx).expect("failed to apply lez-framework");

        for path in [
            "Cargo.toml",
            "crates/lez-client-gen/Cargo.toml",
            "methods/guest/Cargo.toml",
        ] {
            assert!(
                target.join(path).exists(),
                "missing expected manifest in generated project: {path}"
            );
        }

        for path in [
            "Cargo.toml.template",
            "crates/lez-client-gen/Cargo.toml.template",
            "methods/guest/Cargo.toml.template",
        ] {
            assert!(
                !target.join(path).exists(),
                "template manifest should not leak into output: {path}"
            );
        }
    }

    #[test]
    fn overlay_renders_tokens_and_leaves_no_unresolved_placeholders() {
        let temp = mk_temp_dir();
        let target = temp.path();
        let ctx = OverlayRenderContext {
            crate_name: "example-name",
            lez_pin: "deadbeef",
        };

        apply_overlay(target, "default", &ctx).expect("failed to apply default overlay");

        let cargo = fs::read_to_string(target.join("Cargo.toml"))
            .expect("failed to read generated Cargo.toml");
        assert!(cargo.contains("name = \"example-name\""));
        assert!(cargo.contains("rev = \"deadbeef\""));
        assert!(!cargo.contains("{{"));
    }

    #[test]
    fn generated_cargo_toml_does_not_self_patch_logos_blockchain() {
        // Regression guard: an earlier templates change pinned every
        // `logos-blockchain-*` crate via a `[patch."<lb-url>"]` table that
        // pointed back at the same git URL. Cargo treats a self-source patch
        // as a no-op and refuses to resolve, breaking `lgs build` on every
        // freshly-scaffolded project. The user-side build-script panic the
        // patch was meant to mitigate is now handled by
        // `circuits::ensure_circuits_for_subprocess` (which exports
        // `LOGOS_BLOCKCHAIN_CIRCUITS` and bypasses the version check inside
        // every `logos-blockchain` rev's circuits-utils crate).
        for variant in ["default", "lez-framework"] {
            let temp = mk_temp_dir();
            let target = temp.path();
            let ctx = OverlayRenderContext {
                crate_name: "my-app",
                lez_pin: "abc123",
            };
            apply_overlay(target, variant, &ctx)
                .unwrap_or_else(|e| panic!("apply_overlay({variant}) failed: {e}"));
            let cargo = fs::read_to_string(target.join("Cargo.toml"))
                .expect("failed to read generated Cargo.toml");
            assert!(
                !cargo.contains(
                    "[patch.\"https://github.com/logos-blockchain/logos-blockchain.git\"]"
                ),
                "{variant}: generated Cargo.toml must not self-patch the logos-blockchain git URL; got:\n{cargo}"
            );
            // Final guard: the rendered Cargo.toml must still parse as TOML.
            toml_edit::DocumentMut::from_str(&cargo).unwrap_or_else(|e| {
                panic!("{variant}: generated Cargo.toml is not valid TOML: {e}\n---\n{cargo}")
            });
        }
    }

    #[test]
    fn static_files_match_template_content_after_overlay() {
        let temp = mk_temp_dir();
        let target = temp.path();
        let ctx = OverlayRenderContext {
            crate_name: "my-app",
            lez_pin: "abc123",
        };

        apply_overlay(target, "default", &ctx).expect("failed to apply default overlay");

        let env_text = fs::read_to_string(target.join(".env.local"))
            .expect("failed to read generated .env.local");
        assert_eq!(env_text, include_str!("../../templates/default/.env.local"));

        let commands_text = fs::read_to_string(target.join(".scaffold/commands.md"))
            .expect("failed to read generated .scaffold/commands.md");
        assert_eq!(
            commands_text,
            include_str!("../../templates/default/.scaffold/commands.md")
        );

        let runner_text = fs::read_to_string(target.join("src/bin/run_hello_world.rs"))
            .expect("failed to read generated run_hello_world.rs");
        assert_eq!(
            runner_text,
            include_str!("../../templates/default/src/bin/run_hello_world.rs")
        );
    }

    #[test]
    fn gitignore_includes_scaffold_and_is_idempotent() {
        let temp = mk_temp_dir();
        let target = temp.path();
        let ctx = OverlayRenderContext {
            crate_name: "my-app",
            lez_pin: "abc123",
        };

        apply_overlay(target, "default", &ctx).expect("failed to apply default overlay");

        let gitignore = fs::read_to_string(target.join(".gitignore"))
            .expect("failed to read generated .gitignore");
        assert!(
            gitignore.lines().any(|l| l.trim() == ".scaffold"),
            ".gitignore should contain .scaffold, got: {gitignore:?}"
        );
        assert!(
            gitignore.lines().any(|l| l.trim() == ".env.local"),
            ".gitignore should contain .env.local, got: {gitignore:?}"
        );

        apply_overlay(target, "default", &ctx).expect("second overlay should succeed");
        let gitignore_after = fs::read_to_string(target.join(".gitignore"))
            .expect("failed to read .gitignore after second overlay");
        let scaffold_count = gitignore_after
            .lines()
            .filter(|l| l.trim() == ".scaffold")
            .count();
        let env_local_count = gitignore_after
            .lines()
            .filter(|l| l.trim() == ".env.local")
            .count();
        assert_eq!(
            scaffold_count, 1,
            "idempotent overlay must not duplicate .scaffold"
        );
        assert_eq!(
            env_local_count, 1,
            "idempotent overlay must not duplicate .env.local"
        );
    }

    #[test]
    fn render_fails_on_unresolved_placeholder() {
        let ctx = OverlayRenderContext {
            crate_name: "my-app",
            lez_pin: "abc123",
        };

        let err = render_template_text("name = \"{{unknown_token}}\"", &ctx)
            .expect_err("expected unresolved placeholder to fail");
        assert!(
            err.to_string().contains("unresolved template token"),
            "unexpected error: {err}"
        );
    }
}
