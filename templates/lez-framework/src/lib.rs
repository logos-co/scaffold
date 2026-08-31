#[allow(dead_code)]
pub mod runner_support {
    use nssa::{AccountId, program::Program};

    pub fn parse_account_id(raw: &str) -> AccountId {
        let normalized = raw
            .strip_prefix("Public/")
            .or_else(|| raw.strip_prefix("Private/"))
            .unwrap_or(raw);

        normalized
            .parse()
            .unwrap_or_else(|err| panic!("invalid account_id `{raw}`: {err}"))
    }

    pub fn load_program(program_path: Option<&str>, embedded_elf: &[u8], label: &str) -> Program {
        let bytes = if let Some(path) = program_path {
            std::fs::read(path)
                .unwrap_or_else(|err| panic!("failed to read {label} binary at `{path}`: {err}"))
        } else {
            embedded_elf.to_vec()
        };

        Program::new(bytes).unwrap_or_else(|err| panic!("failed to parse {label} program: {err}"))
    }
}

// Host-side program definition for IDL extraction and testing.
// The guest binary (methods/guest) handles zkvm execution.
use nssa_core::account::{AccountWithMetadata, Data};
use spel_framework::prelude::*;

#[lez_program]
mod lez_counter {
    #[allow(unused_imports)]
    use super::*;

    #[instruction]
    pub fn initialize(
        #[account(init, pda = literal("counter"))]
        mut counter: AccountWithMetadata,
        #[account(signer)]
        authority: AccountWithMetadata,
    ) -> SpelResult {
        // Keep this body in step with methods/guest/src/bin/lez_counter.rs: this
        // copy is what `build idl` extracts the IDL from, the guest is what the
        // sequencer executes, and they must describe the same program.
        counter.account.data = Data::try_from(0u64.to_le_bytes().to_vec()).map_err(|_| {
            SpelError::SerializationError {
                message: "counter must be an 8-byte little-endian u64".to_string(),
            }
        })?;

        Ok(SpelOutput::execute(vec![counter, authority], vec![]))
    }

    #[instruction]
    pub fn increment(
        #[account(mut, pda = literal("counter"))]
        mut counter: AccountWithMetadata,
        #[account(signer)]
        authority: AccountWithMetadata,
        amount: u64,
    ) -> SpelResult {
        // The counter lives in `data`, not `balance`. LEZ enforces conservation of
        // total balance across a transaction, so `balance += amount` minted tokens
        // and every `increment` was rejected at the execution check with
        // `InvalidProgramBehavior(ExecutionValidationFailed(MismatchedTotalBalance …))`.
        // `initialize` seeds exactly 8 bytes, so any other length means the account
        // was never initialized or its layout drifted. Convert the whole buffer
        // rather than its first 8 bytes: a prefix decode would accept a longer,
        // drifted account and silently ignore the trailing bytes, and treating a
        // short one as zero would restart the count and hide the corruption.
        let current = <[u8; 8]>::try_from(&counter.account.data[..])
            .map(u64::from_le_bytes)
            .map_err(|_| SpelError::AccountNotInitialized { account_index: 0 })?;

        let next = current
            .checked_add(amount)
            .ok_or_else(|| SpelError::Overflow {
                operation: format!("counter {current} + {amount}"),
            })?;

        counter.account.data = Data::try_from(next.to_le_bytes().to_vec()).map_err(|_| {
            SpelError::SerializationError {
                message: "counter must be an 8-byte little-endian u64".to_string(),
            }
        })?;

        Ok(SpelOutput::execute(vec![counter, authority], vec![]))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn __lssa_idl_print() {
        println!("--- LSSA IDL BEGIN lez_counter ---");
        println!("{}", super::PROGRAM_IDL_JSON);
        println!("--- LSSA IDL END lez_counter ---");
    }
}
