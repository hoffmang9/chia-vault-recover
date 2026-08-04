//! End-to-end recovery workflows shared by CLI and GUI.

use std::path::Path;

use chia_protocol::Coin;
use chia_puzzle_types::Proof;
use clvm_utils::TreeHash;

use crate::chain::ChainClient;
use crate::config::VaultConfig;
use crate::error::{Error, Result};
use crate::keys::MnemonicWordCount;
use crate::network::Network;
use crate::recovery::{
    FinishRecoveryParams, InspectReport, StartRecoveryParams, StartRecoveryResult, VaultPhase,
    finish_recovery, inspect_vault, start_recovery,
};
use crate::vault::{get_vault_internals, recovery_state_hashes};

#[derive(Debug, Clone)]
pub struct ResolvedVault {
    pub coin: Coin,
    pub proof: Proof,
    pub phase: VaultPhase,
}

/// Find the unspent vault singleton and its lineage proof.
///
/// If `post_recovery` is set, also checks the expected RECOVERY puzzle hash when READY is absent.
pub async fn resolve_vault(
    client: &ChainClient,
    config: &VaultConfig,
    post_recovery: Option<&VaultConfig>,
) -> Result<ResolvedVault> {
    let launcher_id = config.launcher_id_bytes()?;
    let keys = config.to_vault_keys()?;
    let ready = get_vault_internals(launcher_id, &keys)?;

    if let Some(coin) = client
        .find_unspent_by_puzzle_hash(ready.full_puzzle_hash)
        .await?
    {
        let proof = client.lineage_proof_for_coin(&coin).await?;
        return Ok(ResolvedVault {
            coin,
            proof,
            phase: VaultPhase::Ready,
        });
    }

    if let Some(post) = post_recovery {
        let post_ready = get_vault_internals(launcher_id, &post.to_vault_keys()?)?;
        // Prefer coin.amount when we find it; for lookup we try amount=1 first (vault singleton).
        let hashes = recovery_state_hashes(launcher_id, &keys, post_ready.inner_puzzle_hash, 1)?;
        if let Some(coin) = client
            .find_unspent_by_puzzle_hash(hashes.internals.full_puzzle_hash)
            .await?
        {
            // Recompute with actual amount if it differs (rare for vaults).
            let hashes = if coin.amount != 1 {
                recovery_state_hashes(
                    launcher_id,
                    &keys,
                    post_ready.inner_puzzle_hash,
                    coin.amount,
                )?
            } else {
                hashes
            };
            if coin.puzzle_hash != hashes.internals.full_puzzle_hash {
                return Err(Error::msg(
                    "found coin amount implies a different RECOVERY puzzle hash",
                ));
            }
            let proof = client.lineage_proof_for_coin(&coin).await?;
            return Ok(ResolvedVault {
                coin,
                proof,
                phase: VaultPhase::InRecovery,
            });
        }
    }

    Err(Error::msg(
        "vault singleton not found on chain (checked READY; checked RECOVERY if post-recovery config given)",
    ))
}

pub async fn inspect(
    client: &ChainClient,
    config: &VaultConfig,
    post_recovery: Option<&VaultConfig>,
) -> Result<InspectReport> {
    let launcher_id = config.launcher_id_bytes()?;
    let keys = config.to_vault_keys()?;
    let ready = get_vault_internals(launcher_id, &keys)?;

    let mut coin = client
        .find_unspent_by_puzzle_hash(ready.full_puzzle_hash)
        .await?;

    let post_inner: Option<TreeHash> = if let Some(post) = post_recovery {
        let post_ready = get_vault_internals(launcher_id, &post.to_vault_keys()?)?;
        if coin.is_none() {
            let amount = 1u64;
            let hashes =
                recovery_state_hashes(launcher_id, &keys, post_ready.inner_puzzle_hash, amount)?;
            coin = client
                .find_unspent_by_puzzle_hash(hashes.internals.full_puzzle_hash)
                .await?;
        }
        Some(post_ready.inner_puzzle_hash)
    } else {
        None
    };

    inspect_vault(config, coin, post_inner)
}

pub struct StartWorkflow<'a> {
    pub config: &'a VaultConfig,
    pub recovery_mnemonic: &'a str,
    pub new_custody_mnemonic: &'a str,
    pub new_recovery_mnemonic: Option<&'a str>,
    pub new_clawback_timelock: Option<u64>,
    pub new_word_count: MnemonicWordCount,
    pub network: Network,
    pub out_config: &'a Path,
}

pub async fn start(client: &ChainClient, params: StartWorkflow<'_>) -> Result<StartRecoveryResult> {
    let resolved = resolve_vault(client, params.config, None).await?;
    if resolved.phase != VaultPhase::Ready {
        return Err(Error::msg(
            "vault is not in READY state; cannot start recovery",
        ));
    }

    let result = start_recovery(StartRecoveryParams {
        config: params.config,
        vault_coin: resolved.coin,
        lineage_proof: resolved.proof,
        recovery_mnemonic: params.recovery_mnemonic,
        new_custody_mnemonic: params.new_custody_mnemonic,
        new_recovery_mnemonic: params.new_recovery_mnemonic,
        new_clawback_timelock: params.new_clawback_timelock,
        new_word_count: params.new_word_count,
        network: params.network,
    })?;

    result.post_recovery_config.save(params.out_config)?;
    client.push_tx(&result.spend_bundle).await?;
    Ok(result)
}

pub async fn finish(
    client: &ChainClient,
    config: &VaultConfig,
    post_recovery: &VaultConfig,
    network: Network,
) -> Result<String> {
    let resolved = resolve_vault(client, config, Some(post_recovery)).await?;
    if resolved.phase != VaultPhase::InRecovery {
        return Err(Error::msg(
            "vault is not in RECOVERY state; cannot finish (did start confirm? is post-recovery config correct?)",
        ));
    }

    let bundle = finish_recovery(FinishRecoveryParams {
        config,
        post_recovery_config: post_recovery,
        vault_coin: resolved.coin,
        lineage_proof: resolved.proof,
        network,
    })?;
    client.push_tx(&bundle).await
}
