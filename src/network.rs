use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Network {
    #[default]
    Mainnet,
    Testnet11,
}

impl Network {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mainnet => "mainnet",
            Self::Testnet11 => "testnet11",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "mainnet" => Some(Self::Mainnet),
            "testnet11" | "testnet" => Some(Self::Testnet11),
            _ => None,
        }
    }

    pub fn coinset_url(self) -> &'static str {
        match self {
            Self::Mainnet => "https://api.coinset.org",
            Self::Testnet11 => "https://testnet11.api.coinset.org",
        }
    }

    pub fn genesis_challenge(self) -> [u8; 32] {
        match self {
            Self::Mainnet => chia_sdk_types::MAINNET_CONSTANTS
                .agg_sig_me_additional_data
                .to_bytes(),
            Self::Testnet11 => chia_sdk_types::TESTNET11_CONSTANTS
                .agg_sig_me_additional_data
                .to_bytes(),
        }
    }
}

impl fmt::Display for Network {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Backend {
    Coinset,
    /// Full node RPC base URL, e.g. `https://localhost:8555`
    FullNode {
        url: String,
    },
}

impl Backend {
    pub fn coinset() -> Self {
        Self::Coinset
    }
}
