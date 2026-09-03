//! Bech32m address encode/decode (xch1 / txch1).

use bech32::{FromBase32, ToBase32, Variant};
use chia_protocol::Bytes32;

use crate::error::{Error, Result};
use crate::network::Network;

pub fn decode_address(addr: &str) -> Result<(String, Bytes32)> {
    let addr = addr.trim();
    let (hrp, data, variant) =
        bech32::decode(addr).map_err(|e| Error::msg(format!("invalid address: {e}")))?;
    if variant != Variant::Bech32m {
        return Err(Error::msg("address must be bech32m (xch1… / txch1…)"));
    }
    let bytes = Vec::<u8>::from_base32(&data)
        .map_err(|e| Error::msg(format!("invalid address payload: {e}")))?;
    if bytes.len() != 32 {
        return Err(Error::msg("address payload is not a 32-byte puzzle hash"));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Ok((hrp, Bytes32::new(arr)))
}

pub fn encode_address(puzzle_hash: Bytes32, hrp: &str) -> Result<String> {
    bech32::encode(hrp, puzzle_hash.to_bytes().to_base32(), Variant::Bech32m)
        .map_err(|e| Error::msg(format!("encode address: {e}")))
}

pub fn network_from_address_prefix(hrp: &str) -> Option<Network> {
    match hrp {
        "xch" => Some(Network::Mainnet),
        "txch" => Some(Network::Testnet11),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_xch() {
        let ph = Bytes32::new([0xab; 32]);
        let addr = encode_address(ph, "xch").unwrap();
        assert!(addr.starts_with("xch1"));
        let (hrp, decoded) = decode_address(&addr).unwrap();
        assert_eq!(hrp, "xch");
        assert_eq!(decoded, ph);
    }
}
