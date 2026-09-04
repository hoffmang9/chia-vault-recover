//! On-disk lookup cache shared by the GUI and CLI.
//!
//! Stores a [found vault](crate::discover::FoundVault) so a later run can skip the
//! chain walk. The recovery phrase is never written. A clawback timelock is stored
//! only when the user supplies one: as a hint until the phrase proves it, then as
//! verified.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use chia_protocol::{Bytes32, Coin};
use clvm_utils::TreeHash;
use serde::{Deserialize, Serialize};

use crate::config::{
    VaultConfigSide, config_members_from_keys, keys_from_config_members, parse_bytes32,
};
use crate::discover::{
    DEFAULT_TIMELOCK_CANDIDATES, DiscoveredCustodyPath, FoundVault, ReconstructedVault,
    reconstruct_from_candidates,
};
use crate::error::{Error, Result};
use crate::network::Network;

const CACHE_VERSION: u32 = 1;
const CACHE_ENV: &str = "CHIA_VAULT_RECOVER_CACHE";

/// A clawback timelock the user entered, with whether the chain has confirmed it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CachedClawback {
    pub secs: u64,
    pub verified: bool,
}

/// One cached vault: chain facts plus optional clawback.
#[derive(Debug, Clone)]
pub struct CachedLookup {
    pub receive_address: String,
    pub network: Network,
    pub found: FoundVault,
    pub clawback: Option<CachedClawback>,
}

/// Result of optionally checking clawback after a lookup is already on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmClawback {
    Hint { secs: u64 },
    Verified { secs: u64, matches_current: bool },
}

/// Persisted lookups keyed by Receive address (or launcher hex).
pub struct LookupCache {
    path: PathBuf,
    file: CacheFile,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheFile {
    version: u32,
    last_address: Option<String>,
    #[serde(default)]
    vaults: BTreeMap<String, VaultRecord>,
}

impl Default for CacheFile {
    fn default() -> Self {
        Self {
            version: CACHE_VERSION,
            last_address: None,
            vaults: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VaultRecord {
    receive_address: String,
    network: String,
    launcher_id: String,
    launcher_source: String,
    custody: VaultConfigSide,
    current_coin: CoinRecord,
    ancestor_puzzle_hashes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    clawback_secs: Option<u64>,
    #[serde(default)]
    clawback_verified: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CoinRecord {
    parent_coin_info: String,
    puzzle_hash: String,
    amount: u64,
}

impl LookupCache {
    pub fn default_path() -> PathBuf {
        if let Ok(path) = std::env::var(CACHE_ENV) {
            let trimmed = path.trim();
            if !trimmed.is_empty() {
                return PathBuf::from(trimmed);
            }
        }
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("chia-vault-recover")
            .join("lookup-cache.json")
    }

    pub fn empty(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            file: CacheFile::default(),
        }
    }

    pub fn open() -> Result<Self> {
        Self::open_at(Self::default_path())
    }

    pub fn open_at(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if !path.exists() {
            return Ok(Self::empty(path));
        }
        let text = fs::read_to_string(&path)?;
        let file: CacheFile = serde_json::from_str(&text)?;
        if file.version != CACHE_VERSION {
            return Err(Error::msg(format!(
                "unsupported lookup cache version {} (expected {CACHE_VERSION})",
                file.version
            )));
        }
        Ok(Self { path, file })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn last(&self) -> Result<Option<CachedLookup>> {
        let Some(key) = &self.file.last_address else {
            return Ok(None);
        };
        self.get(key)
    }

    pub fn get(&self, address: &str) -> Result<Option<CachedLookup>> {
        let Some(record) = self.file.vaults.get(&cache_key(address)) else {
            return Ok(None);
        };
        Ok(Some(record.to_cached()?))
    }

    pub fn record_found(
        &mut self,
        address: &str,
        network: Network,
        found: &FoundVault,
    ) -> Result<()> {
        let key = cache_key(address);
        let keep = self.file.vaults.get(&key).and_then(|existing| {
            if existing.launcher_id == hex_id(&found.launcher_id) {
                Some((existing.clawback_secs, existing.clawback_verified))
            } else {
                None
            }
        });
        let mut record = VaultRecord::from_found(address, network, found);
        if let Some((secs, verified)) = keep {
            record.clawback_secs = secs;
            record.clawback_verified = verified;
        }
        self.file.vaults.insert(key.clone(), record);
        self.file.last_address = Some(key);
        self.save()
    }

    pub fn record_clawback(&mut self, address: &str, secs: u64, verified: bool) -> Result<()> {
        let key = cache_key(address);
        let Some(record) = self.file.vaults.get_mut(&key) else {
            return Err(Error::msg(
                "no cached lookup for this address; look up the vault first",
            ));
        };
        if verified {
            record.clawback_secs = Some(secs);
            record.clawback_verified = true;
        } else if record.clawback_verified && record.clawback_secs == Some(secs) {
            // Same value already proven; keep verified.
        } else {
            record.clawback_secs = Some(secs);
            record.clawback_verified = false;
        }
        self.file.last_address = Some(key);
        self.save()
    }

    /// Optional post-lookup check. Phrase is used only in memory.
    pub fn confirm_clawback(
        &mut self,
        address: &str,
        found: &FoundVault,
        clawback: Option<u64>,
        recovery_mnemonic: Option<&str>,
    ) -> Result<ConfirmClawback> {
        let words = recovery_mnemonic.map(str::trim).filter(|s| !s.is_empty());
        match (words, clawback) {
            (None, None) => Err(Error::msg(
                "enter a clawback window and/or the recovery phrase to check now, or skip and do this later",
            )),
            (None, Some(secs)) => {
                self.record_clawback(address, secs, false)?;
                Ok(ConfirmClawback::Hint { secs })
            }
            (Some(words), clawback) => {
                let candidates = reconstruct_candidates(clawback, None);
                let rebuilt = reconstruct_from_candidates(found, words, &candidates)?;
                self.record_verified(&rebuilt, address)?;
                Ok(ConfirmClawback::Verified {
                    secs: rebuilt.config.recovery.clawback_timelock,
                    matches_current: rebuilt.matches_current,
                })
            }
        }
    }

    pub fn record_verified(&mut self, rebuilt: &ReconstructedVault, address: &str) -> Result<()> {
        self.record_clawback(address, rebuilt.config.recovery.clawback_timelock, true)
    }

    fn save(&mut self) -> Result<()> {
        self.file.version = CACHE_VERSION;
        if let Some(parent) = self.path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }
        fs::write(&self.path, serde_json::to_string_pretty(&self.file)?)?;
        Ok(())
    }
}

/// Candidates for [`reconstruct_from_candidates`].
///
/// An explicit value is tried alone. A verified cache entry is tried alone. A hint
/// is tried first, then the usual Cloud Wallet defaults.
pub fn reconstruct_candidates(typed: Option<u64>, cached: Option<&CachedClawback>) -> Vec<u64> {
    if let Some(secs) = typed {
        return vec![secs];
    }
    match cached {
        Some(cached) if cached.verified => vec![cached.secs],
        Some(cached) => hint_then_defaults(cached.secs),
        None => DEFAULT_TIMELOCK_CANDIDATES.to_vec(),
    }
}

fn hint_then_defaults(hint: u64) -> Vec<u64> {
    let mut out = vec![hint];
    for &secs in DEFAULT_TIMELOCK_CANDIDATES {
        if secs != hint {
            out.push(secs);
        }
    }
    out
}

fn cache_key(address: &str) -> String {
    address.trim().to_ascii_lowercase()
}

fn hex_id(id: &Bytes32) -> String {
    format!("0x{}", hex::encode(id))
}

impl VaultRecord {
    fn from_found(address: &str, network: Network, found: &FoundVault) -> Self {
        Self {
            receive_address: address.trim().to_string(),
            network: network.as_str().to_string(),
            launcher_id: hex_id(&found.launcher_id),
            launcher_source: found.launcher_source.clone(),
            custody: VaultConfigSide {
                threshold: 1,
                members: config_members_from_keys(
                    &found.custody.members,
                    &found.custody.vault_launcher_ids,
                ),
                hash: Some(format!("0x{}", hex::encode(found.custody.custody_hash))),
            },
            current_coin: CoinRecord {
                parent_coin_info: hex_id(&found.current_coin.parent_coin_info),
                puzzle_hash: hex_id(&found.current_coin.puzzle_hash),
                amount: found.current_coin.amount,
            },
            ancestor_puzzle_hashes: found.ancestor_puzzle_hashes.iter().map(hex_id).collect(),
            clawback_secs: None,
            clawback_verified: false,
        }
    }

    fn to_cached(&self) -> Result<CachedLookup> {
        let network = Network::parse(&self.network).ok_or_else(|| {
            Error::msg(format!("lookup cache has unknown network {}", self.network))
        })?;
        let launcher_id = parse_bytes32(&self.launcher_id)?;
        let custody_hash = parse_bytes32(
            self.custody
                .hash
                .as_deref()
                .ok_or_else(|| Error::msg("lookup cache custody is missing its path hash"))?,
        )?;
        let (members, vault_launcher_ids) = keys_from_config_members(&self.custody.members)?;
        let current_coin = Coin::new(
            parse_bytes32(&self.current_coin.parent_coin_info)?,
            parse_bytes32(&self.current_coin.puzzle_hash)?,
            self.current_coin.amount,
        );
        let mut ancestor_puzzle_hashes = Vec::new();
        for ph in &self.ancestor_puzzle_hashes {
            ancestor_puzzle_hashes.push(parse_bytes32(ph)?);
        }
        let clawback = self.clawback_secs.map(|secs| CachedClawback {
            secs,
            verified: self.clawback_verified,
        });
        Ok(CachedLookup {
            receive_address: self.receive_address.clone(),
            network,
            found: FoundVault {
                launcher_id,
                launcher_source: self.launcher_source.clone(),
                custody: DiscoveredCustodyPath {
                    custody_hash: TreeHash::from(custody_hash),
                    members,
                    vault_launcher_ids,
                },
                current_coin,
                ancestor_puzzle_hashes,
            },
            clawback,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chia_protocol::Bytes32;
    use clvm_utils::TreeHash;

    fn found(launcher: u8) -> FoundVault {
        FoundVault {
            launcher_id: Bytes32::new([launcher; 32]),
            launcher_source: "address xch1test".into(),
            custody: DiscoveredCustodyPath {
                custody_hash: TreeHash::from(Bytes32::new([0x11; 32])),
                members: vec![],
                vault_launcher_ids: vec![],
            },
            current_coin: Coin::new(Bytes32::new([0x22; 32]), Bytes32::new([0x33; 32]), 1),
            ancestor_puzzle_hashes: vec![Bytes32::new([0x33; 32])],
        }
    }

    fn temp_cache() -> (LookupCache, PathBuf) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "cvr-lookup-cache-{}-{}.json",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_file(&path);
        (LookupCache::empty(&path), path)
    }

    #[test]
    fn roundtrip_found_without_clawback() {
        let (mut cache, path) = temp_cache();
        cache
            .record_found("XCH1ABC", Network::Mainnet, &found(0xaa))
            .unwrap();
        drop(cache);

        let loaded = LookupCache::open_at(&path).unwrap();
        let entry = loaded.get("xch1abc").unwrap().expect("cached");
        assert_eq!(entry.receive_address, "XCH1ABC");
        assert_eq!(entry.network, Network::Mainnet);
        assert_eq!(entry.found.launcher_id, Bytes32::new([0xaa; 32]));
        assert!(entry.clawback.is_none());
        assert!(!fs::read_to_string(&path).unwrap().contains("mnemonic"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn hint_then_verify_same_value() {
        let (mut cache, path) = temp_cache();
        cache
            .record_found("xch1abc", Network::Mainnet, &found(0xaa))
            .unwrap();
        let hint = cache
            .confirm_clawback("xch1abc", &found(0xaa), Some(43_200), None)
            .unwrap();
        assert_eq!(hint, ConfirmClawback::Hint { secs: 43_200 });
        let entry = cache.get("xch1abc").unwrap().unwrap();
        assert_eq!(
            entry.clawback,
            Some(CachedClawback {
                secs: 43_200,
                verified: false
            })
        );

        cache.record_clawback("xch1abc", 43_200, true).unwrap();
        let entry = cache.get("xch1abc").unwrap().unwrap();
        assert_eq!(
            entry.clawback,
            Some(CachedClawback {
                secs: 43_200,
                verified: true
            })
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn rellookup_same_launcher_keeps_clawback() {
        let (mut cache, path) = temp_cache();
        cache
            .record_found("xch1abc", Network::Mainnet, &found(0xaa))
            .unwrap();
        cache.record_clawback("xch1abc", 43_200, true).unwrap();
        cache
            .record_found("xch1abc", Network::Mainnet, &found(0xaa))
            .unwrap();
        let entry = cache.get("xch1abc").unwrap().unwrap();
        assert_eq!(
            entry.clawback,
            Some(CachedClawback {
                secs: 43_200,
                verified: true
            })
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn rellookup_new_launcher_clears_clawback() {
        let (mut cache, path) = temp_cache();
        cache
            .record_found("xch1abc", Network::Mainnet, &found(0xaa))
            .unwrap();
        cache.record_clawback("xch1abc", 43_200, true).unwrap();
        cache
            .record_found("xch1abc", Network::Mainnet, &found(0xbb))
            .unwrap();
        let entry = cache.get("xch1abc").unwrap().unwrap();
        assert!(entry.clawback.is_none());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn last_is_most_recent_address() {
        let (mut cache, path) = temp_cache();
        cache
            .record_found("xch1aaa", Network::Mainnet, &found(0xaa))
            .unwrap();
        cache
            .record_found("txch1bbb", Network::Testnet11, &found(0xbb))
            .unwrap();
        let last = cache.last().unwrap().unwrap();
        assert_eq!(last.receive_address, "txch1bbb");
        assert_eq!(last.network, Network::Testnet11);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn reconstruct_candidates_typed_verified_hint() {
        let verified = CachedClawback {
            secs: 99,
            verified: true,
        };
        let hint = CachedClawback {
            secs: 7,
            verified: false,
        };
        assert_eq!(reconstruct_candidates(Some(5), Some(&verified)), vec![5]);
        assert_eq!(reconstruct_candidates(None, Some(&verified)), vec![99]);
        let hinted = reconstruct_candidates(None, Some(&hint));
        assert_eq!(hinted[0], 7);
        assert!(hinted.contains(&43_200));
        assert_eq!(
            reconstruct_candidates(None, None),
            DEFAULT_TIMELOCK_CANDIDATES
        );
    }

    #[test]
    fn confirm_requires_something() {
        let (mut cache, path) = temp_cache();
        cache
            .record_found("xch1abc", Network::Mainnet, &found(0xaa))
            .unwrap();
        let err = cache
            .confirm_clawback("xch1abc", &found(0xaa), None, Some("   "))
            .unwrap_err();
        assert!(err.to_string().contains("later"));
        let _ = fs::remove_file(path);
    }
}
