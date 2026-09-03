//! End-user guidance for the address-first lookup flow.

use chia_protocol::Bytes32;

/// Launcher known from a successful resolve (not from the address alone).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownLauncher {
    pub id: Bytes32,
    pub source: String,
}

/// Why chain lookup could not rebuild the vault layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LookupGap {
    /// Address unused, never spent, or parent spends did not reveal a launcher.
    LauncherNotFound,
    /// Launcher is known but the singleton has never been spent (eve only).
    SingletonNeverSpent(KnownLauncher),
    /// Singleton spends exist but none used the current custody path.
    NoCustodySpend(KnownLauncher),
}

impl LookupGap {
    pub fn headline(&self) -> &'static str {
        match self {
            Self::LauncherNotFound => {
                "Could not find this vault’s launcher id from the address alone."
            }
            Self::SingletonNeverSpent(_) => {
                "Found the launcher, but this vault has never been spent with the current setup."
            }
            Self::NoCustodySpend(_) => {
                "Found the launcher, but no previous custody spend is on chain."
            }
        }
    }

    pub fn detail(&self) -> &'static str {
        match self {
            Self::LauncherNotFound => {
                "A Cloud Wallet receive address does not contain the launcher id. \
                 The tool can only recover it from a spent coin at that address \
                 (or from a parent vault spend)."
            }
            Self::SingletonNeverSpent(_) => {
                "An unspent eve singleton does not reveal the custody path. \
                 You need one send from the vault, or a vault-config JSON."
            }
            Self::NoCustodySpend(_) => {
                "Recovery-only spends are not enough. A send that uses the vault’s \
                 passkey (custody) publishes the layout this tool needs."
            }
        }
    }

    pub fn known_launcher(&self) -> Option<&KnownLauncher> {
        match self {
            Self::LauncherNotFound => None,
            Self::SingletonNeverSpent(launcher) | Self::NoCustodySpend(launcher) => Some(launcher),
        }
    }
}

/// What to do when chain lookup cannot replace `vault-config-*.json`.
pub fn fallback_guidance(gap: &LookupGap) -> String {
    format!("{}\n{}\n\n{}", gap.headline(), gap.detail(), FALLBACK_STEPS)
}

pub const FALLBACK_STEPS: &str = "\
A vault-config JSON is required unless the vault has already published its layout on chain.

If you can still open this vault at https://vault.chia.net:

1. Preferred — send any amount from the vault back to the same Receive address \
(or any address you control). A self-send is enough. Cloud Wallet spends the vault \
singleton with your passkey, which publishes the launcher id and custody path. \
Wait until that transaction confirms, then look up the same address again.

2. Or download the public vault-config JSON without sending: while logged in, \
open DevTools → Console (macOS: Option-Command-J; Windows/Linux: Ctrl+Shift+J), \
paste scripts/download-vault-config.js, and press Enter. Then run \
`chia-vault-recover inspect --config vault-config-….json`.

If you cannot access Cloud Wallet, you need a vault-config-*.json you saved earlier.";

pub const LOOKUP_SUCCESS_NO_JSON: &str =
    "Vault layout rebuilt from chain. You do not need a vault-config-*.json download.";

pub const LOOKUP_READY_FOR_PHRASE: &str = "\
Launcher and custody path are on chain. You do not need a vault-config-*.json download. \
Enter the Cloud Wallet recovery phrase to rebuild the public layout and continue.";

pub fn reconstruct_success_guidance(matches_current: bool) -> String {
    let note = if matches_current {
        "Reconstructed config matches the current unspent singleton (READY)."
    } else {
        "Reconstructed config matches a previous singleton state (vault may be in RECOVERY)."
    };
    format!("{LOOKUP_SUCCESS_NO_JSON} {note}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launcher() -> KnownLauncher {
        KnownLauncher {
            id: Bytes32::new([0xaa; 32]),
            source: "test".into(),
        }
    }

    #[test]
    fn fallback_mentions_self_send_and_script() {
        for gap in [
            LookupGap::LauncherNotFound,
            LookupGap::SingletonNeverSpent(launcher()),
            LookupGap::NoCustodySpend(launcher()),
        ] {
            let text = fallback_guidance(&gap);
            assert!(text.contains("self-send"), "{gap:?}");
            assert!(text.contains("download-vault-config.js"), "{gap:?}");
            assert!(text.contains("vault.chia.net"), "{gap:?}");
        }
    }

    #[test]
    fn launcher_only_on_gaps_that_resolved_it() {
        assert!(LookupGap::LauncherNotFound.known_launcher().is_none());
        assert_eq!(
            LookupGap::SingletonNeverSpent(launcher())
                .known_launcher()
                .map(|l| l.id),
            Some(Bytes32::new([0xaa; 32]))
        );
    }
}
