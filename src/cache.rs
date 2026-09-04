//! On-disk lookup cache shared by the GUI and CLI.
//!
//! One current found vault. The recovery phrase is never written. A clawback
//! guess is stored only when the user supplies one. Missing or unreadable files
//! are treated as empty.

use std::fs;
use std::path::{Path, PathBuf};

use chia_protocol::{Bytes32, Coin};
use clvm_utils::TreeHash;
use serde::{Deserialize, Serialize};

use crate::config::{
    VaultConfigMember, config_members_from_keys, keys_from_config_members, parse_bytes32,
};
use crate::discover::{ClawbackGuess, DiscoveredCustodyPath, FoundVault};
use crate::error::{Error, Result};
use crate::network::Network;

const CACHE_ENV: &str = "CHIA_VAULT_RECOVER_CACHE";

/// The last successful lookup, plus an optional clawback guess.
#[derive(Debug, Clone)]
pub struct CachedLookup {
    pub receive_address: String,
    pub network: Network,
    pub found: FoundVault,
    pub clawback: ClawbackGuess,
}

impl CachedLookup {
    pub fn new(address: &str, network: Network, found: FoundVault) -> Self {
        Self {
            receive_address: address.trim().to_string(),
            network,
            found,
            clawback: ClawbackGuess::Unknown,
        }
    }

    /// Replace chain facts. Keeps a clawback guess only when the launcher is unchanged.
    pub fn replace_found(self, address: &str, network: Network, found: FoundVault) -> Self {
        let clawback = if self.found.launcher_id == found.launcher_id {
            self.clawback
        } else {
            ClawbackGuess::Unknown
        };
        Self {
            receive_address: address.trim().to_string(),
            network,
            found,
            clawback,
        }
    }

    pub fn with_clawback(self, clawback: ClawbackGuess) -> Self {
        Self { clawback, ..self }
    }

    fn matches_address(&self, address: &str) -> bool {
        addresses_match(&self.receive_address, address)
    }
}

/// True when two receive addresses name the same vault (trim + case).
pub fn addresses_match(left: &str, right: &str) -> bool {
    cache_key(left) == cache_key(right)
}

/// Single-vault lookup cache.
pub struct LookupCache {
    path: PathBuf,
    entry: Option<CachedLookup>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CacheFile {
    receive_address: String,
    network: String,
    launcher_id: String,
    launcher_source: String,
    custody_hash: String,
    #[serde(default)]
    custody_members: Vec<VaultConfigMember>,
    current_coin: CoinRecord,
    ancestor_puzzle_hashes: Vec<String>,
    #[serde(default, skip_serializing_if = "ClawbackGuess::is_unknown")]
    clawback: ClawbackGuess,
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
        home_dir()
            .join(".chia-vault-recover")
            .join("lookup-cache.json")
    }

    pub fn open() -> Self {
        Self::open_at(Self::default_path())
    }

    pub fn open_at(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref().to_path_buf();
        let entry = fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<CacheFile>(&text).ok())
            .and_then(|file| file.into_cached().ok());
        Self { path, entry }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn current(&self) -> Option<&CachedLookup> {
        self.entry.as_ref()
    }

    pub fn matching(&self, address: &str) -> Option<&CachedLookup> {
        self.entry
            .as_ref()
            .filter(|entry| entry.matches_address(address))
    }

    pub fn store(&mut self, entry: CachedLookup) -> Result<()> {
        let file = CacheFile::from_cached(&entry);
        if let Some(parent) = self.path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }
        fs::write(&self.path, serde_json::to_string_pretty(&file)?)?;
        self.entry = Some(entry);
        Ok(())
    }
}

fn cache_key(address: &str) -> String {
    address.trim().to_ascii_lowercase()
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn hex_id(id: &Bytes32) -> String {
    format!("0x{}", hex::encode(id))
}

impl CacheFile {
    fn from_cached(entry: &CachedLookup) -> Self {
        Self {
            receive_address: entry.receive_address.clone(),
            network: entry.network.as_str().to_string(),
            launcher_id: hex_id(&entry.found.launcher_id),
            launcher_source: entry.found.launcher_source.clone(),
            custody_hash: format!("0x{}", hex::encode(entry.found.custody.custody_hash)),
            custody_members: config_members_from_keys(
                &entry.found.custody.members,
                &entry.found.custody.vault_launcher_ids,
            ),
            current_coin: CoinRecord {
                parent_coin_info: hex_id(&entry.found.current_coin.parent_coin_info),
                puzzle_hash: hex_id(&entry.found.current_coin.puzzle_hash),
                amount: entry.found.current_coin.amount,
            },
            ancestor_puzzle_hashes: entry
                .found
                .ancestor_puzzle_hashes
                .iter()
                .map(hex_id)
                .collect(),
            clawback: entry.clawback,
        }
    }

    fn into_cached(self) -> Result<CachedLookup> {
        let network = Network::parse(&self.network).ok_or_else(|| {
            Error::msg(format!("lookup cache has unknown network {}", self.network))
        })?;
        let (members, vault_launcher_ids) = keys_from_config_members(&self.custody_members)?;
        let mut ancestor_puzzle_hashes = Vec::new();
        for ph in &self.ancestor_puzzle_hashes {
            ancestor_puzzle_hashes.push(parse_bytes32(ph)?);
        }
        Ok(CachedLookup {
            receive_address: self.receive_address,
            network,
            found: FoundVault {
                launcher_id: parse_bytes32(&self.launcher_id)?,
                launcher_source: self.launcher_source,
                custody: DiscoveredCustodyPath {
                    custody_hash: TreeHash::from(parse_bytes32(&self.custody_hash)?),
                    members,
                    vault_launcher_ids,
                },
                current_coin: Coin::new(
                    parse_bytes32(&self.current_coin.parent_coin_info)?,
                    parse_bytes32(&self.current_coin.puzzle_hash)?,
                    self.current_coin.amount,
                ),
                ancestor_puzzle_hashes,
            },
            clawback: self.clawback,
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

    fn temp_path() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "cvr-lookup-cache-{}-{}.json",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_file(&path);
        path
    }

    #[test]
    fn roundtrip_found_without_clawback() {
        let path = temp_path();
        let mut cache = LookupCache::open_at(&path);
        cache
            .store(CachedLookup::new("XCH1ABC", Network::Mainnet, found(0xaa)))
            .unwrap();
        drop(cache);

        let loaded = LookupCache::open_at(&path);
        let entry = loaded.matching("xch1abc").expect("cached");
        assert_eq!(entry.receive_address, "XCH1ABC");
        assert_eq!(entry.network, Network::Mainnet);
        assert_eq!(entry.found.launcher_id, Bytes32::new([0xaa; 32]));
        assert_eq!(entry.clawback, ClawbackGuess::Unknown);
        assert!(!fs::read_to_string(&path).unwrap().contains("mnemonic"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn store_replaces_the_only_entry() {
        let path = temp_path();
        let mut cache = LookupCache::open_at(&path);
        cache
            .store(CachedLookup::new("xch1aaa", Network::Mainnet, found(0xaa)))
            .unwrap();
        cache
            .store(CachedLookup::new(
                "txch1bbb",
                Network::Testnet11,
                found(0xbb),
            ))
            .unwrap();
        let current = cache.current().unwrap();
        assert_eq!(current.receive_address, "txch1bbb");
        assert!(cache.matching("xch1aaa").is_none());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn addresses_match_trims_and_ignores_case() {
        assert!(addresses_match("  XCH1ABC  ", "xch1abc"));
        assert!(!addresses_match("xch1abc", "xch1abd"));
    }

    #[test]
    fn replace_found_keeps_clawback_for_same_launcher() {
        let kept = CachedLookup::new("xch1abc", Network::Mainnet, found(0xaa))
            .with_clawback(ClawbackGuess::Known(43_200))
            .replace_found("xch1abc", Network::Mainnet, found(0xaa));
        assert_eq!(kept.clawback, ClawbackGuess::Known(43_200));

        let cleared = CachedLookup::new("xch1abc", Network::Mainnet, found(0xaa))
            .with_clawback(ClawbackGuess::Known(43_200))
            .replace_found("xch1abc", Network::Mainnet, found(0xbb));
        assert_eq!(cleared.clawback, ClawbackGuess::Unknown);
    }

    #[test]
    fn unreadable_file_is_empty() {
        let path = temp_path();
        fs::write(&path, "{not json").unwrap();
        assert!(LookupCache::open_at(&path).current().is_none());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn default_path_uses_dot_dir_under_home() {
        if std::env::var(CACHE_ENV)
            .ok()
            .is_some_and(|value| !value.trim().is_empty())
        {
            return;
        }
        let path = LookupCache::default_path();
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some("lookup-cache.json")
        );
        assert_eq!(
            path.parent()
                .and_then(|parent| parent.file_name())
                .and_then(|name| name.to_str()),
            Some(".chia-vault-recover")
        );
    }
}
