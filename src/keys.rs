//! Cloud Wallet–compatible BLS key derivation from BIP39 mnemonics.
//!
//! Matches ent-wallet `apps/app/src/utils/bls.ts`:
//! `mnemonicToSeedSync(words)` with empty passphrase → `AugSchemeMPL.keyGen(seed)`.
//! No CIP-14 / `m/12381/8444/...` derivation path.

use bip39::{Language, Mnemonic};
use chia_bls::{PublicKey, SecretKey, Signature, sign};
use rand::RngCore;

use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MnemonicWordCount {
    Words12,
    Words24,
}

impl MnemonicWordCount {
    fn entropy_bytes(self) -> usize {
        match self {
            Self::Words12 => 16,
            Self::Words24 => 32,
        }
    }
}

#[derive(Debug, Clone)]
pub struct KeyPair {
    pub secret_key: SecretKey,
    pub public_key: PublicKey,
}

#[derive(Debug, Clone)]
pub struct GeneratedMnemonic {
    pub words: String,
    pub key_pair: KeyPair,
}

/// Derive the Cloud Wallet recovery (or new custody) key from a BIP39 mnemonic.
pub fn key_from_mnemonic(mnemonic: &str) -> Result<KeyPair> {
    let mnemonic = Mnemonic::parse_in_normalized(Language::English, mnemonic.trim())?;
    let seed = mnemonic.to_seed("");
    let secret_key = SecretKey::from_seed(&seed);
    let public_key = secret_key.public_key();
    Ok(KeyPair {
        secret_key,
        public_key,
    })
}

/// Generate a new mnemonic (default 24 words) and its Cloud Wallet–style key pair.
pub fn generate_mnemonic(word_count: MnemonicWordCount) -> Result<GeneratedMnemonic> {
    let mut entropy = vec![0u8; word_count.entropy_bytes()];
    rand::thread_rng().fill_bytes(&mut entropy);
    let mnemonic = Mnemonic::from_entropy_in(Language::English, &entropy)?;
    let words = mnemonic.to_string();
    let key_pair = key_from_mnemonic(&words)?;
    Ok(GeneratedMnemonic { words, key_pair })
}

pub fn sign_message(secret_key: &SecretKey, message: &[u8]) -> Signature {
    sign(secret_key, message)
}

pub fn parse_signature(hex: &str) -> Result<Signature> {
    let bytes = parse_hex_bytes(hex)?;
    let arr: [u8; 96] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| Error::msg("BLS signature must be 96 bytes"))?;
    Signature::from_bytes(&arr).map_err(|e| Error::msg(format!("invalid signature: {e}")))
}

pub fn parse_hex_bytes(hex_str: &str) -> Result<Vec<u8>> {
    let hex_str = hex_str.trim().trim_start_matches("0x");
    Ok(hex::decode(hex_str)?)
}

pub fn public_key_to_hex(pk: &PublicKey) -> String {
    format!("0x{}", hex::encode(pk.to_bytes()))
}

pub fn parse_bls_public_key(hex_str: &str) -> Result<PublicKey> {
    let bytes = parse_hex_bytes(hex_str)?;
    PublicKey::from_bytes(
        bytes
            .as_slice()
            .try_into()
            .map_err(|_| Error::msg("BLS public key must be 48 bytes"))?,
    )
    .map_err(|e| Error::msg(format!("invalid BLS public key: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_generate_24() {
        let generated = generate_mnemonic(MnemonicWordCount::Words24).unwrap();
        assert_eq!(generated.words.split_whitespace().count(), 24);
        let again = key_from_mnemonic(&generated.words).unwrap();
        assert_eq!(
            again.public_key.to_bytes(),
            generated.key_pair.public_key.to_bytes()
        );
    }

    #[test]
    fn round_trip_generate_12() {
        let generated = generate_mnemonic(MnemonicWordCount::Words12).unwrap();
        assert_eq!(generated.words.split_whitespace().count(), 12);
    }
}
