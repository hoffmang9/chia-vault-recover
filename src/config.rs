//! Cloud Wallet vault-config JSON (export/import format).

use std::fs;
use std::path::Path;

use chia_protocol::Bytes32;
use clvm_utils::TreeHash;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::keys::{KeyPair, parse_bls_public_key, parse_hex_bytes, public_key_to_hex};
use crate::vault::{VaultKeys, VaultMemberKey};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Curve {
    Secp256k1,
    Secp256r1,
    Webauthn,
    Bls12_381,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum KeyType {
    Passkey,
    App,
    AppSoftware,
    RecoveryPhrase,
    Vault,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum VaultConfigMember {
    #[serde(rename = "publicKey")]
    PublicKey {
        #[serde(rename = "publicKey")]
        public_key: String,
        curve: Curve,
        #[serde(rename = "keyType", default)]
        key_type: Option<KeyType>,
    },
    #[serde(rename = "vault")]
    Vault {
        #[serde(rename = "launcherId")]
        launcher_id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultConfigSide {
    pub threshold: u32,
    #[serde(default)]
    pub members: Vec<VaultConfigMember>,
    /// MIPS custody-path hash. Required when `members` is empty (on-chain discovery).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultConfigRecovery {
    pub threshold: u32,
    pub clawback_timelock: u64,
    pub members: Vec<VaultConfigMember>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultConfig {
    pub launcher_id: String,
    pub custody: VaultConfigSide,
    pub recovery: VaultConfigRecovery,
}

impl VaultConfig {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let text = fs::read_to_string(path)?;
        Ok(serde_json::from_str(&text)?)
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let text = serde_json::to_string_pretty(self)?;
        fs::write(path, text)?;
        Ok(())
    }

    pub fn launcher_id_bytes(&self) -> Result<Bytes32> {
        parse_bytes32(&self.launcher_id)
    }

    pub fn to_vault_keys(&self) -> Result<VaultKeys> {
        Ok(VaultKeys {
            custody: side_to_signers(&self.custody)?,
            recovery: recovery_to_signers(&self.recovery)?,
        })
    }

    /// Build a simple post-recovery config: 1-of-1 BLS custody + 1-of-1 BLS recovery.
    /// Does not include private mnemonics.
    pub fn from_bls_pair(
        launcher_id: Bytes32,
        custody: &KeyPair,
        recovery: &KeyPair,
        clawback_timelock: u64,
    ) -> Self {
        Self {
            launcher_id: format!("0x{}", hex::encode(launcher_id)),
            custody: VaultConfigSide {
                threshold: 1,
                members: vec![VaultConfigMember::PublicKey {
                    public_key: public_key_to_hex(&custody.public_key),
                    curve: Curve::Bls12_381,
                    key_type: Some(KeyType::RecoveryPhrase),
                }],
                hash: None,
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
        }
    }
}

fn side_to_signers(side: &VaultConfigSide) -> Result<crate::vault::SignerSet> {
    let mut keys = Vec::new();
    let mut vault_launcher_ids = Vec::new();
    for member in &side.members {
        match member {
            VaultConfigMember::PublicKey {
                public_key, curve, ..
            } => keys.push(member_key(*curve, public_key)?),
            VaultConfigMember::Vault { launcher_id } => {
                vault_launcher_ids.push(parse_bytes32(launcher_id)?);
            }
        }
    }
    let hash_override = side
        .hash
        .as_ref()
        .map(|h| parse_bytes32(h).map(TreeHash::from))
        .transpose()?;
    if keys.is_empty() && vault_launcher_ids.is_empty() && hash_override.is_none() {
        return Err(Error::msg(
            "custody/recovery side has no members and no hash",
        ));
    }
    Ok(crate::vault::SignerSet {
        keys,
        vault_launcher_ids,
        threshold: side.threshold as usize,
        hash_override,
    })
}

fn recovery_to_signers(side: &VaultConfigRecovery) -> Result<crate::vault::RecoverySignerSet> {
    Ok(crate::vault::RecoverySignerSet {
        set: side_to_signers(&VaultConfigSide {
            threshold: side.threshold,
            members: side.members.clone(),
            hash: None,
        })?,
        clawback_timelock: side.clawback_timelock,
    })
}

fn member_key(curve: Curve, public_key_hex: &str) -> Result<VaultMemberKey> {
    match curve {
        Curve::Bls12_381 => Ok(VaultMemberKey::Bls(parse_bls_public_key(public_key_hex)?)),
        Curve::Secp256r1 | Curve::Webauthn => {
            let bytes = parse_hex_bytes(public_key_hex)?;
            let arr: [u8; 33] = bytes
                .as_slice()
                .try_into()
                .map_err(|_| Error::msg("secp/passkey public key must be 33 compressed bytes"))?;
            let pk = chia_secp::R1PublicKey::from_bytes(&arr)
                .map_err(|e| Error::msg(format!("invalid R1 public key: {e}")))?;
            Ok(if curve == Curve::Webauthn {
                VaultMemberKey::Passkey(pk)
            } else {
                VaultMemberKey::R1(pk)
            })
        }
        Curve::Secp256k1 => {
            let bytes = parse_hex_bytes(public_key_hex)?;
            let arr: [u8; 33] = bytes
                .as_slice()
                .try_into()
                .map_err(|_| Error::msg("secp256k1 public key must be 33 compressed bytes"))?;
            let pk = chia_secp::K1PublicKey::from_bytes(&arr)
                .map_err(|e| Error::msg(format!("invalid K1 public key: {e}")))?;
            Ok(VaultMemberKey::K1(pk))
        }
    }
}

pub fn parse_bytes32(hex_str: &str) -> Result<Bytes32> {
    let bytes = parse_hex_bytes(hex_str)?;
    let arr: [u8; 32] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| Error::msg("expected 32-byte hex value"))?;
    Ok(Bytes32::new(arr))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cloud_wallet_shaped_config() {
        let json = r#"{
          "launcherId": "0x1111111111111111111111111111111111111111111111111111111111111111",
          "custody": {
            "threshold": 1,
            "members": [
              {
                "type": "publicKey",
                "publicKey": "0x02aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "curve": "WEBAUTHN",
                "keyType": "PASSKEY"
              }
            ]
          },
          "recovery": {
            "threshold": 1,
            "clawbackTimelock": 43200,
            "members": [
              {
                "type": "publicKey",
                "publicKey": "0xb6b6b6b6b6b6b6b6b6b6b6b6b6b6b6b6b6b6b6b6b6b6b6b6b6b6b6b6b6b6b6b6b6b6b6b6b6b6b6b6b6b6b6b6b6b6b6b6",
                "curve": "BLS12_381",
                "keyType": "RECOVERY_PHRASE"
              }
            ]
          }
        }"#;
        let config: VaultConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.recovery.clawback_timelock, 43200);
        assert!(matches!(
            config.custody.members[0],
            VaultConfigMember::PublicKey {
                curve: Curve::Webauthn,
                ..
            }
        ));
    }
}
