//! End-to-end recovery workflows shared by CLI and GUI.

use std::path::Path;

use chia_protocol::Coin;
use chia_puzzle_types::Proof;
use clvm_utils::TreeHash;

use crate::chain::ChainClient;
use crate::config::VaultConfig;
use crate::discover::{
    DEFAULT_TIMELOCK_CANDIDATES, DiscoverReport, config_matches_puzzle_hash,
    custody_from_vault_spend, reconstruct_config,
};
use crate::error::{Error, Result};
use crate::guidance::{
    LOOKUP_READY_FOR_PHRASE, LOOKUP_SUCCESS_NO_JSON, LookupGap, fallback_guidance,
};
use crate::keys::MnemonicWordCount;
use crate::locate::{parse_vault_locator, resolve_launcher_id};
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

/// Result of looking up a vault from its receive address (or launcher id).
#[derive(Debug, Clone)]
pub enum LookupReport {
    /// Custody path matched; public vault-config was rebuilt. JSON download is not needed.
    Reconstructed(DiscoverReport),
    /// Launcher and custody spend are on chain; recovery phrase is required to finish rebuild.
    ReadyForPhrase {
        launcher_id: chia_protocol::Bytes32,
        launcher_source: String,
        guidance: String,
    },
    /// Chain does not yet reveal enough. User should self-send or download vault-config.
    NeedFallback {
        launcher_id: Option<chia_protocol::Bytes32>,
        launcher_source: Option<String>,
        reason: String,
        guidance: String,
    },
}

/// Address-first lookup: resolve launcher, see if a vault-config JSON is needed.
///
/// Pass `recovery_mnemonic` to rebuild the public layout when the chain has it.
pub async fn lookup(
    client: &ChainClient,
    vault: &str,
    recovery_mnemonic: Option<&str>,
    clawback_timelock: Option<u64>,
) -> Result<LookupReport> {
    let locator = parse_vault_locator(vault)?;
    let resolved = match resolve_launcher_id(client, &locator).await {
        Ok(resolved) => resolved,
        Err(e) => {
            return Ok(LookupReport::NeedFallback {
                launcher_id: None,
                launcher_source: None,
                reason: e.to_string(),
                guidance: fallback_guidance(LookupGap::LauncherNotFound),
            });
        }
    };
    let launcher_id = resolved.launcher_id;
    let launcher_source = resolved.source;

    let (spent, current) = match client.walk_singleton_chain(launcher_id).await {
        Ok(chain) => chain,
        Err(e) => {
            return Ok(LookupReport::NeedFallback {
                launcher_id: Some(launcher_id),
                launcher_source: Some(launcher_source),
                reason: e.to_string(),
                guidance: fallback_guidance(LookupGap::LauncherNotFound),
            });
        }
    };
    if spent.is_empty() {
        return Ok(LookupReport::NeedFallback {
            launcher_id: Some(launcher_id),
            launcher_source: Some(launcher_source),
            reason: LookupGap::SingletonNeverSpent.headline().to_string(),
            guidance: fallback_guidance(LookupGap::SingletonNeverSpent),
        });
    }

    let mut last_parse_error = None;
    let mut custody = None;
    for record in spent.iter().rev() {
        let Some(spend) = client
            .get_spend(record.coin.coin_id(), record.spent_block_index)
            .await?
        else {
            continue;
        };
        match custody_from_vault_spend(&spend) {
            Ok(Some(path)) => {
                custody = Some(path);
                break;
            }
            Ok(None) => {}
            Err(e) => last_parse_error = Some(e),
        }
    }
    let Some(custody) = custody else {
        let reason = last_parse_error
            .map(|e| e.to_string())
            .unwrap_or_else(|| LookupGap::NoCustodySpend.headline().to_string());
        return Ok(LookupReport::NeedFallback {
            launcher_id: Some(launcher_id),
            launcher_source: Some(launcher_source),
            reason,
            guidance: fallback_guidance(LookupGap::NoCustodySpend),
        });
    };

    let Some(recovery_mnemonic) = recovery_mnemonic.filter(|s| !s.trim().is_empty()) else {
        return Ok(LookupReport::ReadyForPhrase {
            launcher_id,
            launcher_source,
            guidance: LOOKUP_READY_FOR_PHRASE.to_string(),
        });
    };

    let candidates: Vec<u64> = if let Some(secs) = clawback_timelock {
        vec![secs]
    } else {
        DEFAULT_TIMELOCK_CANDIDATES.to_vec()
    };

    for &timelock in &candidates {
        let config = reconstruct_config(launcher_id, &custody, recovery_mnemonic, timelock)?;
        let ready_match = config_matches_puzzle_hash(&config, current.coin.puzzle_hash)?;
        let ancestor_match = spent
            .iter()
            .any(|r| config_matches_puzzle_hash(&config, r.coin.puzzle_hash).unwrap_or(false));
        if ready_match || ancestor_match {
            let members_complete = !config.custody.members.is_empty();
            let match_note = if ready_match {
                "Reconstructed config matches the current unspent singleton (READY)."
            } else {
                "Reconstructed config matches a previous singleton state (vault may be in RECOVERY)."
            };
            return Ok(LookupReport::Reconstructed(DiscoverReport {
                config,
                custody_hash: custody.custody_hash,
                clawback_timelock: timelock,
                current_coin: current.coin,
                members_complete,
                guidance: format!("{LOOKUP_SUCCESS_NO_JSON} {match_note}"),
                launcher_source,
            }));
        }
    }

    Err(Error::msg(format!(
        "found custody hash 0x{} but no candidate timelock produced a matching vault puzzle hash. \
         Pass --clawback-secs explicitly (Cloud Wallet default is 43200)",
        hex::encode(custody.custody_hash)
    )))
}

/// Walk the singleton, parse a previous custody spend, and rebuild a vault-config.
///
/// `vault` is a receive address (`xch1…` / `txch1…`) or a launcher id.
pub async fn discover(
    client: &ChainClient,
    vault: &str,
    recovery_mnemonic: &str,
    clawback_timelock: Option<u64>,
) -> Result<DiscoverReport> {
    match lookup(client, vault, Some(recovery_mnemonic), clawback_timelock).await? {
        LookupReport::Reconstructed(report) => Ok(report),
        LookupReport::ReadyForPhrase { .. } => Err(Error::msg(
            "recovery mnemonic required to rebuild vault layout",
        )),
        LookupReport::NeedFallback {
            reason, guidance, ..
        } => Err(Error::msg(format!("{reason}\n\n{guidance}"))),
    }
}
