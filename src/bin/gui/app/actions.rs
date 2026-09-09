//! Chain / workflow actions for the wizard.

use chia_vault_recover::config::VaultConfig;
use chia_vault_recover::discover::{ClawbackCheck, FoundVault, check_clawback};
use chia_vault_recover::keys::MnemonicWordCount;
use chia_vault_recover::locate::client_for_vault;
use chia_vault_recover::network::Network;
use chia_vault_recover::recovery::VaultPhase;
use chia_vault_recover::workflow::{self, LookupReport, StartWorkflow};
use chia_vault_recover::LookupGap;

use super::{App, Phase, runtime};

impl App {
    pub(super) fn apply_found(&mut self, found: FoundVault, network: Network) {
        let launcher = hex::encode(found.launcher_id);
        let source = found.launcher_source.clone();
        let address = self.vault_address.trim().to_string();
        self.clear_session_disk();
        self.generated_recovery_mnemonic = None;
        match self.cache.persist_found(&address, network, found) {
            Ok(_) => {
                self.network_mainnet = matches!(network, Network::Mainnet);
                self.phase = Phase::Start;
                self.set_ok(format!(
                    "Launcher 0x{launcher} from {source}. Lookup saved. You can close the app and come back to Start, or continue now."
                ));
            }
            Err(e) => {
                self.set_err(format!(
                    "Launcher 0x{launcher} from {source}. Could not save lookup cache: {e}"
                ));
            }
        }
    }

    pub(super) fn apply_fallback(&mut self, gap: LookupGap, network: Network) {
        self.network_mainnet = matches!(network, Network::Mainnet);
        let launcher = match gap.known_launcher() {
            Some(known) => format!(" launcher 0x{} ({}).", hex::encode(known.id), known.source),
            None => String::new(),
        };
        self.set_err(format!(
            "Lookup needs a self-send or vault-config.{launcher} {}",
            gap.headline()
        ));
        self.phase = Phase::Fallback(gap);
    }

    pub(super) fn run_lookup(&mut self) {
        let result = (|| {
            let vault = self.vault_address.trim();
            if vault.is_empty() {
                return Err(chia_vault_recover::Error::msg(
                    "enter the vault Receive address (xch1… / txch1…) first",
                ));
            }
            let (client, network) = client_for_vault(vault, self.network(), &self.backend())?;
            let report = runtime().block_on(workflow::lookup(&client, vault))?;
            match report {
                LookupReport::Found(found) => self.apply_found(found, network),
                LookupReport::NeedFallback(gap) => self.apply_fallback(gap, network),
            }
            Ok::<(), chia_vault_recover::Error>(())
        })();
        if let Err(e) = result {
            self.set_err(format!("Lookup error: {e}"));
        }
    }

    pub(super) fn load_existing_config(&mut self) {
        let result = (|| {
            let config = VaultConfig::load(&self.config_path)?;
            self.clear_session_disk();
            self.detail.clear();
            self.phase = Phase::Start;
            self.set_ok(format!(
                "Loaded {}. launcher {}. Inspect, then Start recovery.",
                self.config_path, config.launcher_id
            ));
            Ok::<(), chia_vault_recover::Error>(())
        })();
        if let Err(e) = result {
            self.set_err(format!("Load config error: {e}"));
        }
    }

    pub(super) fn run_confirm_clawback(&mut self) {
        let result = (|| {
            let Some(entry) = self.cached_vault() else {
                return Err(chia_vault_recover::Error::msg(
                    "look up the vault before checking clawback",
                ));
            };
            let found = entry.found.clone();
            let address = entry.receive_address.clone();
            let words = self.recovery_mnemonic.trim();
            let check = check_clawback(
                &found,
                if words.is_empty() { None } else { Some(words) },
                self.parsed_clawback()?,
            )?;
            self.cache.persist_guess(&address, check.guess())?;
            let message = match check {
                ClawbackCheck::Hint(secs) => {
                    format!(
                        "Saved clawback {secs}s as a hint. Enter the recovery phrase later to confirm it matches the chain. The phrase is never written to disk."
                    )
                }
                ClawbackCheck::Verified(rebuilt) => {
                    let secs = rebuilt.config.recovery.clawback_timelock;
                    self.clawback_secs = secs.to_string();
                    let match_note = if rebuilt.matches_current {
                        "Matches the current unspent singleton."
                    } else {
                        "Matches a previous singleton state."
                    };
                    format!(
                        "Clawback {secs}s confirmed and saved. {match_note} The recovery phrase was not written to disk. You can Start recovery when ready."
                    )
                }
            };
            self.set_ok(message);
            Ok::<(), chia_vault_recover::Error>(())
        })();
        if let Err(e) = result {
            self.set_err(format!("Clawback check: {e}"));
        }
    }

    pub(super) fn run_inspect(&mut self) {
        let result = (|| {
            let config = VaultConfig::load(&self.config_path)?;
            let post = {
                let path = self.post_recovery_path.trim();
                if path.is_empty() || !std::path::Path::new(path).is_file() {
                    None
                } else {
                    Some(VaultConfig::load(path)?)
                }
            };
            let (client, _) = self.chain_client()?;
            let report = runtime().block_on(workflow::inspect(&client, &config, post.as_ref()))?;
            self.detail = report.guidance.clone();
            self.set_ok(match report.phase {
                VaultPhase::InRecovery => format!("Phase: InRecovery — {}", report.guidance),
                phase => format!("Phase: {phase:?}"),
            });
            Ok::<(), chia_vault_recover::Error>(())
        })();
        if let Err(e) = result {
            self.set_err(format!("Inspect error: {e}"));
        }
    }

    pub(super) fn run_start(&mut self) {
        let result = (|| {
            if self.recovery_mnemonic.trim().is_empty() {
                return Err(chia_vault_recover::Error::msg(
                    "enter the Cloud Wallet recovery phrase to start recovery",
                ));
            }
            if self.new_custody_mnemonic.trim().is_empty() {
                return Err(chia_vault_recover::Error::msg(
                    "enter a new custody mnemonic to start recovery",
                ));
            }
            let out = self.resolve_post_path();
            let lookup_out = self.resolve_config_path();
            Self::ensure_parent_dir(&out)?;
            Self::ensure_parent_dir(&lookup_out)?;
            let word_count = if self.generate_12_words {
                MnemonicWordCount::Words12
            } else {
                MnemonicWordCount::Words24
            };
            let new_recovery = {
                let trimmed = self.new_recovery_mnemonic.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed)
                }
            };
            let recovery_mnemonic = self.recovery_mnemonic.trim();
            let new_custody_mnemonic = self.new_custody_mnemonic.trim();
            let typed_clawback = self.parsed_clawback()?;
            let (client, network) = self.chain_client()?;
            let push_start = |config: &VaultConfig| {
                runtime().block_on(workflow::start(
                    &client,
                    StartWorkflow {
                        config,
                        recovery_mnemonic,
                        new_custody_mnemonic,
                        new_recovery_mnemonic: new_recovery,
                        new_clawback_timelock: None,
                        new_word_count: word_count,
                        network,
                        out_config: &out,
                    },
                ))
            };
            let start = if self.cached_vault().is_some() {
                let rebuilt = workflow::rebuild_for_start(
                    &mut self.cache,
                    self.vault_address.trim(),
                    recovery_mnemonic,
                    typed_clawback,
                )?;
                self.clawback_secs = rebuilt.config.recovery.clawback_timelock.to_string();
                rebuilt.config.save(&lookup_out)?;
                self.config_path = lookup_out.display().to_string();
                push_start(&rebuilt.config)?
            } else if self.config_on_disk() {
                let config = VaultConfig::load(&self.config_path)?;
                push_start(&config)?
            } else {
                return Err(chia_vault_recover::Error::msg(
                    "look up the vault or load a vault-config before Start recovery",
                ));
            };
            self.post_recovery_path = out.display().to_string();
            self.generated_recovery_mnemonic = start.generated_recovery_mnemonic.clone();
            self.detail =
                "Vault entering RECOVERY. After the clawback period, click Finish recovery.".into();
            self.set_ok(format!(
                "Start pushed. Wait {}s then Finish. Config: {}",
                start.clawback_timelock,
                out.display()
            ));
            self.begin_wait(start.clawback_timelock);
            self.recovery_mnemonic.clear();
            self.new_custody_mnemonic.clear();
            self.new_recovery_mnemonic.clear();
            Ok::<(), chia_vault_recover::Error>(())
        })();
        if let Err(e) = result {
            self.set_err(format!("Start error: {e}"));
        }
    }

    pub(super) fn run_finish(&mut self) {
        let result = (|| {
            let config = VaultConfig::load(&self.config_path)?;
            let post = VaultConfig::load(&self.post_recovery_path)?;
            let (client, network) = self.chain_client()?;
            let handle = runtime().block_on(workflow::finish(&client, &config, &post, network))?;
            self.clear_session_disk();
            self.generated_recovery_mnemonic = None;
            self.phase = Phase::Done;
            self.set_ok(format!(
                "Finish pushed ({handle}). Vault custody is now the new BLS key."
            ));
            Ok::<(), chia_vault_recover::Error>(())
        })();
        if let Err(e) = result {
            self.set_err(format!("Finish error: {e}"));
        }
    }
}
