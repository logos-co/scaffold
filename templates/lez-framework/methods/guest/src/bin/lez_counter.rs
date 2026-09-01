#![no_main]

use nssa_core::account::Data;
use spel_framework::prelude::*;

#[cfg(not(test))]
risc0_zkvm::guest::entry!(main);

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
        // Start the counter at an explicit zero so `increment` always has a
        // well-formed 8-byte little-endian value to read back.
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
        // `initialize` seeds exactly 8 bytes, so any other length is a fault worth
        // reporting rather than papering over: treating a short buffer as zero
        // would silently restart the count. Empty means the account was never
        // initialized, which is the likely mistake (calling `increment` first);
        // any other length is a corrupt or drifted layout, so keep the two
        // distinguishable. Convert the whole buffer, not its first 8 bytes — a
        // prefix decode would accept a longer account and drop the trailing bytes.
        let current = match &counter.account.data[..] {
            [] => return Err(SpelError::AccountNotInitialized { account_index: 0 }),
            bytes => <[u8; 8]>::try_from(bytes).map(u64::from_le_bytes).map_err(|_| {
                SpelError::SerializationError {
                    message: "counter must be an 8-byte little-endian u64".to_string(),
                }
            })?,
        };

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
