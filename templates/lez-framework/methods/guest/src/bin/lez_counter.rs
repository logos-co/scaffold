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
        // The framework derives each account's auto-claim from its
        // `#[account(...)]` constraints (init+pda -> PDA claim, signer -> none).
        Ok(SpelOutput::execute(vec![counter, authority], vec![]))
    }

    #[instruction]
    pub fn increment(
        #[account(mut, pda = literal("counter"))]
        counter: AccountWithMetadata,
        #[account(signer)]
        authority: AccountWithMetadata,
        amount: u64,
    ) -> SpelResult {
        let mut counter = counter;
        counter.account.balance += amount as u128;
        Ok(SpelOutput::execute(vec![counter, authority], vec![]))
    }
}
