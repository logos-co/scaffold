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
        counter.account.data = Data::try_from(0u64.to_le_bytes().to_vec())
            .expect("an 8-byte counter always fits in Data");

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
        let current = counter
            .account
            .data
            .get(..8)
            .and_then(|head| <[u8; 8]>::try_from(head).ok())
            .map_or(0u64, u64::from_le_bytes);

        counter.account.data = Data::try_from(current.wrapping_add(amount).to_le_bytes().to_vec())
            .expect("an 8-byte counter always fits in Data");

        Ok(SpelOutput::execute(vec![counter, authority], vec![]))
    }
}
