#![no_main]

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
        counter: AccountWithMetadata,
        #[account(signer)]
        authority: AccountWithMetadata,
    ) -> SpelResult {
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
        counter.account.balance += amount as u128;

        Ok(SpelOutput::execute(vec![counter, authority], vec![]))
    }
}
