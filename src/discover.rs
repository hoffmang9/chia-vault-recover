//! Reconstruct vault custody from a previous on-chain spend (Sage fallback).
//!
//! Cloud Wallet never hints the full vault config on-chain. A custody spend of the
//! current 1-of-2 inner puzzle *does* reveal that path: the One-of-N member puzzle
//! hashes to `custody_hash`, and a 1-of-1 member also reveals the public key.
//! Combined with the recovery seed and clawback timelock we can rebuild a config
//! that matches the singleton.

use chia_protocol::{Bytes32, Coin, CoinSpend};
use chia_puzzle_types::singleton::SingletonSolution;
use chia_puzzles::{FORCE_1_OF_2_W_RESTRICTED_VARIABLE_HASH, ONE_OF_N_HASH, RESTRICTIONS_HASH};
use chia_sdk_driver::{Puzzle, SpendContext};
use chia_sdk_types::{
    Mod,
    puzzles::{
        BlsMember, BlsMemberPuzzleAssert, DelegatedPuzzleFeederArgs, DelegatedPuzzleFeederSolution,
        INDEX_WRAPPER_HASH, IndexWrapperArgs, K1Member, K1MemberPuzzleAssert, OneOfNArgs,
        OneOfNSolution, PasskeyMember, PasskeyMemberPuzzleAssert, R1Member, R1MemberPuzzleAssert,
        RestrictionsArgs, SingletonMember, SingletonMemberWithMode,
    },
};
use clvm_traits::FromClvm;
use clvm_utils::{ToTreeHash, TreeHash};
use clvmr::{Allocator, NodePtr};

use crate::config::{
    Curve, KeyType, VaultConfig, VaultConfigMember, VaultConfigRecovery, VaultConfigSide,
};
use crate::error::{Error, Result};
use crate::keys::{key_from_mnemonic, public_key_to_hex};
use crate::vault::{RecoverySignerSet, SignerSet, VaultKeys, VaultMemberKey, get_vault_internals};

/// Common Cloud Wallet / test timelocks, tried when the user does not pass one.
pub const DEFAULT_TIMELOCK_CANDIDATES: &[u64] = &[
    43_200, 86_400, 3_600, 7_200, 21_600, 604_800, 1, 10, 60, 300,
];

#[derive(Debug, Clone)]
pub struct DiscoveredCustodyPath {
    pub custody_hash: TreeHash,
    pub members: Vec<VaultMemberKey>,
    pub vault_launcher_ids: Vec<Bytes32>,
}

#[derive(Debug, Clone)]
pub struct DiscoverReport {
    pub config: VaultConfig,
    pub custody_hash: TreeHash,
    pub clawback_timelock: u64,
    pub current_coin: Coin,
    pub members_complete: bool,
    pub guidance: String,
    /// How the launcher id was obtained (`launcher id 0x…` or `address xch1…`).
    pub launcher_source: String,
}

/// Extract the custody path from a vault singleton spend, if this spend used custody.
pub fn custody_from_vault_spend(spend: &CoinSpend) -> Result<Option<DiscoveredCustodyPath>> {
    let mut ctx = SpendContext::new();
    let puzzle_ptr = ctx
        .alloc(&spend.puzzle_reveal)
        .map_err(|e| Error::msg(format!("alloc puzzle: {e}")))?;
    let solution_ptr = ctx
        .alloc(&spend.solution)
        .map_err(|e| Error::msg(format!("alloc solution: {e}")))?;
    let puzzle = Puzzle::parse(&ctx, puzzle_ptr);
    custody_from_parsed_spend(&ctx, puzzle, solution_ptr)
}

fn custody_from_parsed_spend(
    alloc: &Allocator,
    puzzle: Puzzle,
    solution: NodePtr,
) -> Result<Option<DiscoveredCustodyPath>> {
    let Some(curried) = puzzle.as_curried() else {
        return Ok(None);
    };
    // Singleton top layer.
    let inner_puzzle = {
        use chia_puzzle_types::singleton::SingletonArgs;
        use chia_puzzles::SINGLETON_TOP_LAYER_V1_1_HASH;
        if curried.mod_hash != SINGLETON_TOP_LAYER_V1_1_HASH.into() {
            return Ok(None);
        }
        let args = SingletonArgs::<Puzzle>::from_clvm(alloc, curried.args)
            .map_err(|e| Error::msg(format!("parse singleton args: {e}")))?;
        args.inner_puzzle
    };
    let singleton_sol = SingletonSolution::<NodePtr>::from_clvm(alloc, solution)
        .map_err(|e| Error::msg(format!("parse singleton solution: {e}")))?;

    let (after_index, index_sol) =
        unwrap_index_wrapper(alloc, inner_puzzle, singleton_sol.inner_solution)?;
    let (after_feeder, feeder_sol) = unwrap_delegated_feeder(alloc, after_index, index_sol)?;

    let Some(one_of_n) = after_feeder.as_curried() else {
        return Ok(None);
    };
    if one_of_n.mod_hash != ONE_OF_N_HASH.into() {
        return Ok(None);
    }
    let _args = OneOfNArgs::from_clvm(alloc, one_of_n.args)
        .map_err(|e| Error::msg(format!("parse one-of-n args: {e}")))?;
    let one_sol = OneOfNSolution::<NodePtr, NodePtr>::from_clvm(alloc, feeder_sol)
        .map_err(|e| Error::msg(format!("parse one-of-n solution: {e}")))?;

    let member_puzzle = Puzzle::parse(alloc, one_sol.member_puzzle);
    if member_looks_like_recovery(alloc, member_puzzle) {
        return Ok(None);
    }

    let custody_hash = member_puzzle.tree_hash();
    let (mut members, mut vault_launcher_ids) =
        parse_member_keys(alloc, member_puzzle, one_sol.member_solution);

    if !members.is_empty() || !vault_launcher_ids.is_empty() {
        let trial = SignerSet {
            keys: members.clone(),
            vault_launcher_ids: vault_launcher_ids.clone(),
            threshold: 1,
            hash_override: None,
        };
        if let Ok(computed) = crate::vault::custody_hash_from_set(&trial)
            && computed != custody_hash
        {
            // Incomplete M-of-N: keep the hash, drop partial members.
            members.clear();
            vault_launcher_ids.clear();
        }
    }

    Ok(Some(DiscoveredCustodyPath {
        custody_hash,
        members,
        vault_launcher_ids,
    }))
}

/// Rebuild a vault-config that matches `custody` + recovery seed + timelock.
pub fn reconstruct_config(
    launcher_id: Bytes32,
    custody: &DiscoveredCustodyPath,
    recovery_mnemonic: &str,
    clawback_timelock: u64,
) -> Result<VaultConfig> {
    let recovery = key_from_mnemonic(recovery_mnemonic)?;
    let keys = VaultKeys {
        custody: SignerSet {
            keys: custody.members.clone(),
            vault_launcher_ids: custody.vault_launcher_ids.clone(),
            threshold: 1,
            hash_override: Some(custody.custody_hash),
        },
        recovery: RecoverySignerSet {
            set: SignerSet {
                keys: vec![VaultMemberKey::Bls(recovery.public_key)],
                vault_launcher_ids: vec![],
                threshold: 1,
                hash_override: None,
            },
            clawback_timelock,
        },
    };
    let internals = get_vault_internals(launcher_id, &keys)?;
    if internals.custody_hash != custody.custody_hash {
        return Err(Error::msg(
            "reconstructed custody hash does not match spend",
        ));
    }

    Ok(VaultConfig {
        launcher_id: format!("0x{}", hex::encode(launcher_id)),
        custody: VaultConfigSide {
            threshold: 1,
            members: members_to_config(&custody.members, &custody.vault_launcher_ids),
            hash: Some(format!("0x{}", hex::encode(custody.custody_hash))),
        },
        recovery: VaultConfigRecovery {
            threshold: 1,
            clawback_timelock,
            members: vec![VaultConfigMember::PublicKey {
                public_key: public_key_to_hex(&recovery.public_key),
                curve: Curve::Bls12_381,
                key_type: Some(KeyType::RecoveryPhrase),
            }],
        },
    })
}

pub fn config_matches_puzzle_hash(config: &VaultConfig, puzzle_hash: Bytes32) -> Result<bool> {
    let internals = get_vault_internals(config.launcher_id_bytes()?, &config.to_vault_keys()?)?;
    Ok(internals.full_puzzle_hash == puzzle_hash)
}

fn members_to_config(
    keys: &[VaultMemberKey],
    vault_launcher_ids: &[Bytes32],
) -> Vec<VaultConfigMember> {
    let mut out = Vec::new();
    for key in keys {
        out.push(match key {
            VaultMemberKey::Bls(pk) => VaultConfigMember::PublicKey {
                public_key: public_key_to_hex(pk),
                curve: Curve::Bls12_381,
                key_type: Some(KeyType::RecoveryPhrase),
            },
            VaultMemberKey::K1(pk) => VaultConfigMember::PublicKey {
                public_key: format!("0x{}", hex::encode(pk.to_bytes())),
                curve: Curve::Secp256k1,
                key_type: Some(KeyType::App),
            },
            VaultMemberKey::R1(pk) => VaultConfigMember::PublicKey {
                public_key: format!("0x{}", hex::encode(pk.to_bytes())),
                curve: Curve::Secp256r1,
                key_type: Some(KeyType::App),
            },
            VaultMemberKey::Passkey(pk) => VaultConfigMember::PublicKey {
                public_key: format!("0x{}", hex::encode(pk.to_bytes())),
                curve: Curve::Webauthn,
                key_type: Some(KeyType::Passkey),
            },
        });
    }
    for launcher_id in vault_launcher_ids {
        out.push(VaultConfigMember::Vault {
            launcher_id: format!("0x{}", hex::encode(launcher_id)),
        });
    }
    out
}

fn unwrap_index_wrapper(
    alloc: &Allocator,
    puzzle: Puzzle,
    solution: NodePtr,
) -> Result<(Puzzle, NodePtr)> {
    let Some(curried) = puzzle.as_curried() else {
        return Ok((puzzle, solution));
    };
    if curried.mod_hash != INDEX_WRAPPER_HASH {
        return Ok((puzzle, solution));
    }
    let args = IndexWrapperArgs::<usize, Puzzle>::from_clvm(alloc, curried.args)
        .map_err(|e| Error::msg(format!("parse index wrapper: {e}")))?;
    Ok((args.inner_puzzle, solution))
}

fn unwrap_delegated_feeder(
    alloc: &Allocator,
    puzzle: Puzzle,
    solution: NodePtr,
) -> Result<(Puzzle, NodePtr)> {
    let Some(curried) = puzzle.as_curried() else {
        return Ok((puzzle, solution));
    };
    if curried.mod_hash != chia_puzzles::DELEGATED_PUZZLE_FEEDER_HASH.into() {
        return Ok((puzzle, solution));
    }
    let args = DelegatedPuzzleFeederArgs::<Puzzle>::from_clvm(alloc, curried.args)
        .map_err(|e| Error::msg(format!("parse delegated puzzle feeder: {e}")))?;
    let feeder =
        DelegatedPuzzleFeederSolution::<NodePtr, NodePtr, NodePtr>::from_clvm(alloc, solution)
            .map_err(|e| Error::msg(format!("parse delegated puzzle feeder solution: {e}")))?;
    Ok((args.inner_puzzle, feeder.inner_solution))
}

fn member_looks_like_recovery(alloc: &Allocator, puzzle: Puzzle) -> bool {
    let Ok((inner, _)) = unwrap_index_wrapper(alloc, puzzle, NodePtr::NIL) else {
        return false;
    };
    let Some(curried) = inner.as_curried() else {
        return false;
    };
    if curried.mod_hash != RESTRICTIONS_HASH.into() {
        return false;
    }
    let Ok(args) =
        RestrictionsArgs::<Vec<Puzzle>, Vec<Puzzle>, Puzzle>::from_clvm(alloc, curried.args)
    else {
        return true;
    };
    args.delegated_puzzle_validators.iter().any(|p| {
        p.as_curried()
            .is_some_and(|c| c.mod_hash == FORCE_1_OF_2_W_RESTRICTED_VARIABLE_HASH.into())
    })
}

fn parse_member_keys(
    alloc: &Allocator,
    puzzle: Puzzle,
    solution: NodePtr,
) -> (Vec<VaultMemberKey>, Vec<Bytes32>) {
    let mut keys = Vec::new();
    let mut vaults = Vec::new();
    collect_member_keys(alloc, puzzle, solution, &mut keys, &mut vaults);
    (keys, vaults)
}

fn collect_member_keys(
    alloc: &Allocator,
    puzzle: Puzzle,
    solution: NodePtr,
    keys: &mut Vec<VaultMemberKey>,
    vaults: &mut Vec<Bytes32>,
) {
    let Ok((inner, inner_sol)) = unwrap_index_wrapper(alloc, puzzle, solution) else {
        return;
    };
    let Some(curried) = inner.as_curried() else {
        return;
    };

    if curried.mod_hash == RESTRICTIONS_HASH.into() {
        if let Ok(args) =
            RestrictionsArgs::<Vec<Puzzle>, Vec<Puzzle>, Puzzle>::from_clvm(alloc, curried.args)
        {
            collect_member_keys(alloc, args.inner_puzzle, inner_sol, keys, vaults);
        }
        return;
    }

    if curried.mod_hash == ONE_OF_N_HASH.into()
        && let Ok(one_sol) = OneOfNSolution::<NodePtr, NodePtr>::from_clvm(alloc, inner_sol)
    {
        collect_member_keys(
            alloc,
            Puzzle::parse(alloc, one_sol.member_puzzle),
            one_sol.member_solution,
            keys,
            vaults,
        );
        return;
    }

    if let Some(key) = parse_leaf_member(alloc, inner) {
        match key {
            Leaf::Key(k) => keys.push(k),
            Leaf::Vault(id) => vaults.push(id),
        }
    }
}

enum Leaf {
    Key(VaultMemberKey),
    Vault(Bytes32),
}

fn parse_leaf_member(alloc: &Allocator, puzzle: Puzzle) -> Option<Leaf> {
    let curried = puzzle.as_curried()?;
    let args = curried.args;

    if curried.mod_hash == BlsMember::mod_hash()
        && let Ok(m) = BlsMember::from_clvm(alloc, args)
    {
        return Some(Leaf::Key(VaultMemberKey::Bls(m.public_key)));
    }
    if curried.mod_hash == BlsMemberPuzzleAssert::mod_hash()
        && let Ok(m) = BlsMemberPuzzleAssert::from_clvm(alloc, args)
    {
        return Some(Leaf::Key(VaultMemberKey::Bls(m.public_key)));
    }
    if curried.mod_hash == K1Member::mod_hash()
        && let Ok(m) = K1Member::from_clvm(alloc, args)
    {
        return Some(Leaf::Key(VaultMemberKey::K1(m.public_key)));
    }
    if curried.mod_hash == K1MemberPuzzleAssert::mod_hash()
        && let Ok(m) = K1MemberPuzzleAssert::from_clvm(alloc, args)
    {
        return Some(Leaf::Key(VaultMemberKey::K1(m.public_key)));
    }
    if curried.mod_hash == R1Member::mod_hash()
        && let Ok(m) = R1Member::from_clvm(alloc, args)
    {
        return Some(Leaf::Key(VaultMemberKey::R1(m.public_key)));
    }
    if curried.mod_hash == R1MemberPuzzleAssert::mod_hash()
        && let Ok(m) = R1MemberPuzzleAssert::from_clvm(alloc, args)
    {
        return Some(Leaf::Key(VaultMemberKey::R1(m.public_key)));
    }
    if curried.mod_hash == PasskeyMember::mod_hash()
        && let Ok(m) = PasskeyMember::from_clvm(alloc, args)
    {
        return Some(Leaf::Key(VaultMemberKey::Passkey(m.public_key)));
    }
    if curried.mod_hash == PasskeyMemberPuzzleAssert::mod_hash()
        && let Ok(m) = PasskeyMemberPuzzleAssert::from_clvm(alloc, args)
    {
        return Some(Leaf::Key(VaultMemberKey::Passkey(m.public_key)));
    }
    if curried.mod_hash == SingletonMember::mod_hash()
        && let Ok(m) = SingletonMember::from_clvm(alloc, args)
    {
        return Some(Leaf::Vault(m.singleton_struct.launcher_id));
    }
    if curried.mod_hash == SingletonMemberWithMode::mod_hash()
        && let Ok(m) = SingletonMemberWithMode::from_clvm(alloc, args)
    {
        return Some(Leaf::Vault(m.singleton_struct.launcher_id));
    }
    parse_leaf_from_args(alloc, curried.mod_hash, args)
}

fn parse_leaf_from_args(alloc: &Allocator, mod_hash: TreeHash, args: NodePtr) -> Option<Leaf> {
    use chia_bls::PublicKey;
    use chia_secp::{K1PublicKey, R1PublicKey};

    // Curry args are `(c (q . ARG) rest)`. Unquote the first argument.
    let first = match <(NodePtr, NodePtr)>::from_clvm(alloc, args) {
        Ok((quoted, _)) => match <(NodePtr, NodePtr)>::from_clvm(alloc, quoted) {
            Ok((_, value)) => value,
            Err(_) => quoted,
        },
        Err(_) => args,
    };

    if (mod_hash == BlsMember::mod_hash() || mod_hash == BlsMemberPuzzleAssert::mod_hash())
        && let Ok(pk) =
            PublicKey::from_clvm(alloc, first).or_else(|_| PublicKey::from_clvm(alloc, args))
    {
        return Some(Leaf::Key(VaultMemberKey::Bls(pk)));
    }
    if (mod_hash == K1Member::mod_hash() || mod_hash == K1MemberPuzzleAssert::mod_hash())
        && let Ok(pk) =
            K1PublicKey::from_clvm(alloc, first).or_else(|_| K1PublicKey::from_clvm(alloc, args))
    {
        return Some(Leaf::Key(VaultMemberKey::K1(pk)));
    }
    if (mod_hash == R1Member::mod_hash() || mod_hash == R1MemberPuzzleAssert::mod_hash())
        && let Ok(pk) =
            R1PublicKey::from_clvm(alloc, first).or_else(|_| R1PublicKey::from_clvm(alloc, args))
    {
        return Some(Leaf::Key(VaultMemberKey::R1(pk)));
    }
    if (mod_hash == PasskeyMember::mod_hash() || mod_hash == PasskeyMemberPuzzleAssert::mod_hash())
        && let Ok(pk) =
            R1PublicKey::from_clvm(alloc, first).or_else(|_| R1PublicKey::from_clvm(alloc, args))
    {
        return Some(Leaf::Key(VaultMemberKey::Passkey(pk)));
    }
    None
}

#[cfg(test)]
mod tests {
    use chia_sdk_driver::SpendContext;
    use chia_sdk_types::puzzles::BlsMember;

    use crate::keys::{MnemonicWordCount, generate_mnemonic};

    use super::*;

    #[test]
    fn parse_curried_bls_member() {
        let mut ctx = SpendContext::new();
        let key = generate_mnemonic(MnemonicWordCount::Words12).unwrap();
        let member = BlsMember::new(key.key_pair.public_key);
        let puzzle = ctx.curry(member).unwrap();
        let alloc: &Allocator = &ctx;
        let parsed = Puzzle::parse(alloc, puzzle);
        let leaf = parse_leaf_member(alloc, parsed);
        assert!(
            matches!(leaf, Some(Leaf::Key(VaultMemberKey::Bls(_)))),
            "failed to parse curried BLS member"
        );
    }

    #[test]
    fn parse_index_wrapped_bls_member() {
        let mut ctx = SpendContext::new();
        let key = generate_mnemonic(MnemonicWordCount::Words12).unwrap();
        let inner = ctx.curry(BlsMember::new(key.key_pair.public_key)).unwrap();
        let wrapped = ctx.curry(IndexWrapperArgs::new(0usize, inner)).unwrap();
        let alloc: &Allocator = &ctx;
        let parsed = Puzzle::parse(alloc, wrapped);
        let (mut keys, vaults) = parse_member_keys(alloc, parsed, NodePtr::NIL);
        assert!(vaults.is_empty());
        assert!(
            matches!(keys.first(), Some(VaultMemberKey::Bls(_))),
            "keys={keys:?}"
        );
        keys.clear();
    }
}
