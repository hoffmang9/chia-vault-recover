//! Shared MIPS layer types for address lookup and custody discovery.

use chia_protocol::CoinSpend;
use chia_sdk_driver::{
    DelegatedPuzzleFeederLayer, IndexWrapperLayer, Layer, Puzzle, SingletonLayer,
    SingletonMemberLayer, SpendContext,
};
use clvmr::NodePtr;

use crate::error::Result;

pub type CloudWalletP2 = IndexWrapperLayer<usize, DelegatedPuzzleFeederLayer<SingletonMemberLayer>>;
pub type VaultMipsInner = IndexWrapperLayer<usize, DelegatedPuzzleFeederLayer<Puzzle>>;
pub type VaultSingleton = SingletonLayer<VaultMipsInner>;

pub fn alloc_spend(spend: &CoinSpend) -> Result<(SpendContext, Puzzle, NodePtr)> {
    let mut ctx = SpendContext::new();
    let puzzle_ptr = ctx.alloc(&spend.puzzle_reveal)?;
    let solution_ptr = ctx.alloc(&spend.solution)?;
    let puzzle = Puzzle::parse(&ctx, puzzle_ptr);
    Ok((ctx, puzzle, solution_ptr))
}

/// Peel singleton + index wrapper + delegated feeder.
/// Returns the inner puzzle (typically OneOfN) and its solution.
pub fn peel_vault_mips(
    alloc: &clvmr::Allocator,
    puzzle: Puzzle,
    solution: NodePtr,
) -> Result<Option<(Puzzle, NodePtr)>> {
    let Some(vault) = VaultSingleton::parse_puzzle(alloc, puzzle)? else {
        return Ok(None);
    };
    let sol = VaultSingleton::parse_solution(alloc, solution)?;
    Ok(Some((
        vault.inner_puzzle.inner_puzzle.inner_puzzle,
        sol.inner_solution.inner_solution,
    )))
}

pub fn peel_index_wrapper(alloc: &clvmr::Allocator, puzzle: Puzzle) -> Puzzle {
    IndexWrapperLayer::<usize, Puzzle>::parse_puzzle(alloc, puzzle)
        .ok()
        .flatten()
        .map(|wrapped| wrapped.inner_puzzle)
        .unwrap_or(puzzle)
}
