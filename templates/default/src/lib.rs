#[allow(dead_code)]
pub mod runner_support {
    use anyhow::{Context, Result};
    use lee::{AccountId, program::Program};

    pub fn parse_account_id(raw: &str) -> Result<AccountId> {
        let normalized = raw
            .strip_prefix("Public/")
            .or_else(|| raw.strip_prefix("Private/"))
            .unwrap_or(raw);

        normalized
            .parse()
            .with_context(|| format!("invalid account_id `{raw}` (expected base58 account id)"))
    }

    pub fn load_program(program_path: Option<&str>, embedded_elf: &[u8], label: &str) -> Result<Program> {
        let bytes = if let Some(path) = program_path {
            std::fs::read(path)
                .with_context(|| format!("failed to read {label} binary at `{path}`"))?
        } else {
            embedded_elf.to_vec()
        };

        // LEZ v0.2.x takes the ELF as a `Cow<'static, [u8]>`.
        Program::new(bytes.into()).with_context(|| format!("failed to parse {label} program"))
    }
}
