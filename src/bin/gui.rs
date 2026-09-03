//! Address-first egui GUI for vault recovery.

use std::path::PathBuf;
use std::sync::OnceLock;

use chia_vault_recover::chain::ChainClient;
use chia_vault_recover::config::VaultConfig;
use chia_vault_recover::discover::{FoundVault, reconstruct};
use chia_vault_recover::guidance::{
    LOOKUP_READY_FOR_PHRASE, fallback_guidance, reconstruct_success_guidance,
};
use chia_vault_recover::keys::MnemonicWordCount;
use chia_vault_recover::locate::client_for_vault;
use chia_vault_recover::network::{Backend, Network};
use chia_vault_recover::recovery::VaultPhase;
use chia_vault_recover::workflow::{self, LookupReport, StartWorkflow};
use eframe::egui;

fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("tokio runtime")
    })
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([760.0, 860.0])
            .with_title("Chia Vault Recover"),
        ..Default::default()
    };
    eframe::run_native(
        "Chia Vault Recover",
        options,
        Box::new(|_cc| Ok(Box::new(App::default()))),
    )
}

const INTRO_GUIDANCE: &str =
    "Enter the vault Receive address (xch1… / txch1…) from Cloud Wallet, then click Look up vault.";
const LOADED_CONFIG_GUIDANCE: &str =
    "Using a downloaded vault-config JSON. Inspect, then Start recovery.";
const AFTER_START_GUIDANCE: &str =
    "Vault entering RECOVERY. After the clawback period, click Finish recovery.";

enum AppStep {
    NeedLookup,
    Found {
        vault_input: String,
        found: FoundVault,
    },
    NeedFallback(chia_vault_recover::LookupGap),
    ReadyFromLookup {
        vault_input: String,
        found: FoundVault,
        next: String,
    },
    ReadyFromFile {
        next: String,
    },
}

struct App {
    vault_address: String,
    config_path: String,
    post_recovery_path: String,
    clawback_secs: String,
    recovery_mnemonic: String,
    new_custody_mnemonic: String,
    new_recovery_mnemonic: String,
    generate_12_words: bool,
    network_mainnet: bool,
    full_node_url: String,
    status: String,
    generated_recovery_mnemonic: Option<String>,
    step: AppStep,
}

impl Default for App {
    fn default() -> Self {
        Self {
            vault_address: String::new(),
            config_path: String::new(),
            post_recovery_path: String::new(),
            clawback_secs: String::new(),
            recovery_mnemonic: String::new(),
            new_custody_mnemonic: String::new(),
            new_recovery_mnemonic: String::new(),
            generate_12_words: false,
            network_mainnet: true,
            full_node_url: String::new(),
            status: String::new(),
            generated_recovery_mnemonic: None,
            step: AppStep::NeedLookup,
        }
    }
}

impl App {
    fn network(&self) -> Network {
        if self.network_mainnet {
            Network::Mainnet
        } else {
            Network::Testnet11
        }
    }

    fn backend(&self) -> Backend {
        let url = self.full_node_url.trim();
        if url.is_empty() {
            Backend::Coinset
        } else {
            Backend::FullNode {
                url: url.to_string(),
            }
        }
    }

    fn chain_client(&self) -> chia_vault_recover::error::Result<(ChainClient, Network)> {
        let vault = self.vault_address.trim();
        if vault.is_empty() {
            Ok((
                ChainClient::new(self.network(), &self.backend()),
                self.network(),
            ))
        } else {
            client_for_vault(vault, self.network(), &self.backend())
        }
    }

    fn layout_ready(&self) -> bool {
        matches!(
            self.step,
            AppStep::ReadyFromLookup { .. } | AppStep::ReadyFromFile { .. }
        )
    }

    fn guidance(&self) -> String {
        match &self.step {
            AppStep::NeedLookup => INTRO_GUIDANCE.to_string(),
            AppStep::Found { .. } => LOOKUP_READY_FOR_PHRASE.to_string(),
            AppStep::NeedFallback(gap) => fallback_guidance(gap),
            AppStep::ReadyFromLookup { next, .. } | AppStep::ReadyFromFile { next } => next.clone(),
        }
    }

    fn set_ready_next(&mut self, next: String) {
        match &mut self.step {
            AppStep::ReadyFromLookup { next: slot, .. } | AppStep::ReadyFromFile { next: slot } => {
                *slot = next;
            }
            _ => {}
        }
    }

    fn cached_found(&self, vault: &str) -> Option<&FoundVault> {
        match &self.step {
            AppStep::Found {
                vault_input, found, ..
            }
            | AppStep::ReadyFromLookup {
                vault_input, found, ..
            } if vault_input == vault => Some(found),
            _ => None,
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.heading("Chia Vault Recover");
                ui.label("Start with the bech32m Receive address. The tool looks up the launcher and tells you if a vault-config JSON is needed.");
                ui.separator();

                ui.strong("1. Vault address");
                ui.horizontal(|ui| {
                    ui.label("Receive address (xch1… / txch1…):");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.vault_address)
                            .desired_width(f32::INFINITY)
                            .hint_text("xch1…"),
                    );
                });
                ui.checkbox(&mut self.network_mainnet, "Mainnet (unchecked = testnet11; xch1/txch1 overrides this)");
                ui.horizontal(|ui| {
                    ui.label("Full node URL (optional; empty = coinset):");
                    ui.text_edit_singleline(&mut self.full_node_url);
                });
                ui.horizontal(|ui| {
                    if ui.button("Look up vault").clicked() {
                        self.run_lookup();
                    }
                    ui.label("Uses the recovery phrase below if you have already pasted it.");
                });

                ui.add_space(8.0);
                ui.strong("2. Recovery phrase");
                ui.label("Cloud Wallet recovery passphrase. Needed to rebuild the public layout and to start delayed recovery.");
                ui.add(
                    egui::TextEdit::multiline(&mut self.recovery_mnemonic)
                        .desired_rows(2)
                        .desired_width(f32::INFINITY),
                );
                ui.horizontal(|ui| {
                    ui.label("Clawback seconds (optional; empty = try defaults):");
                    ui.text_edit_singleline(&mut self.clawback_secs);
                });

                ui.add_space(8.0);
                ui.collapsing("Already have a vault-config JSON?", |ui| {
                    ui.label("Only needed if lookup says the chain does not yet show this vault’s layout.");
                    ui.horizontal(|ui| {
                        ui.label("Vault config:");
                        ui.text_edit_singleline(&mut self.config_path);
                        if ui.button("Browse…").clicked()
                            && let Some(path) = rfd::FileDialog::new().pick_file()
                        {
                            self.config_path = path.display().to_string();
                            self.step = AppStep::ReadyFromFile {
                                next: LOADED_CONFIG_GUIDANCE.to_string(),
                            };
                        }
                    });
                    if ui.button("Load config").clicked() {
                        self.load_existing_config();
                    }
                });

                ui.add_space(8.0);
                ui.strong("3. New keys and delayed recovery");
                ui.label("New custody mnemonic (required to start):");
                ui.add(
                    egui::TextEdit::multiline(&mut self.new_custody_mnemonic)
                        .desired_rows(2)
                        .desired_width(f32::INFINITY),
                );
                ui.label("New recovery mnemonic (optional — leave empty to auto-generate):");
                ui.add(
                    egui::TextEdit::multiline(&mut self.new_recovery_mnemonic)
                        .desired_rows(2)
                        .desired_width(f32::INFINITY),
                );
                ui.checkbox(
                    &mut self.generate_12_words,
                    "Generate 12-word mnemonics (default 24)",
                );
                ui.horizontal(|ui| {
                    ui.label("Post-recovery config:");
                    ui.text_edit_singleline(&mut self.post_recovery_path);
                    if ui.button("Browse…").clicked()
                        && let Some(path) = rfd::FileDialog::new().pick_file()
                    {
                        self.post_recovery_path = path.display().to_string();
                    }
                });

                ui.separator();
                ui.horizontal(|ui| {
                    if ui.add_enabled(self.layout_ready(), egui::Button::new("Inspect")).clicked() {
                        self.run_inspect();
                    }
                    if ui.add_enabled(self.layout_ready(), egui::Button::new("Start recovery")).clicked() {
                        self.run_start();
                    }
                    if ui.button("Finish recovery").clicked() {
                        self.run_finish();
                    }
                });
                if !self.layout_ready() {
                    ui.label("Look up the vault (or load a vault-config JSON) before Inspect / Start.");
                }

                if let Some(words) = &self.generated_recovery_mnemonic {
                    ui.separator();
                    ui.colored_label(
                        egui::Color32::from_rgb(180, 60, 60),
                        "SAVE THIS NEW RECOVERY MNEMONIC (not written to config):",
                    );
                    ui.monospace(words);
                    if ui.button("Copy recovery mnemonic").clicked() {
                        ui.ctx().copy_text(words.clone());
                        self.status = "Copied recovery mnemonic to clipboard.".into();
                    }
                }

                let guidance = self.guidance();
                if !guidance.is_empty() {
                    ui.separator();
                    ui.strong("What next");
                    ui.label(guidance);
                }
                if !self.status.is_empty() {
                    ui.separator();
                    ui.label(&self.status);
                }
            });
        });
    }
}

impl App {
    fn clawback(&self) -> chia_vault_recover::error::Result<Option<u64>> {
        let trimmed = self.clawback_secs.trim();
        if trimmed.is_empty() {
            Ok(None)
        } else {
            trimmed
                .parse::<u64>()
                .map(Some)
                .map_err(|_| chia_vault_recover::Error::msg("clawback seconds must be a number"))
        }
    }

    fn apply_reconstructed(
        &mut self,
        rebuilt: chia_vault_recover::ReconstructedVault,
        network: Network,
    ) -> chia_vault_recover::error::Result<()> {
        self.network_mainnet = matches!(network, Network::Mainnet);
        let out = if self.config_path.is_empty() {
            PathBuf::from("vault-config.json")
        } else {
            PathBuf::from(&self.config_path)
        };
        rebuilt.config.save(&out)?;
        self.config_path = out.display().to_string();
        self.status = format!(
            "Looked up launcher 0x{} ({}). Wrote {}. You do not need a vault-config download.",
            hex::encode(rebuilt.config.launcher_id_bytes()?),
            rebuilt.found.launcher_source,
            out.display()
        );
        self.step = AppStep::ReadyFromLookup {
            vault_input: self.vault_address.trim().to_string(),
            found: rebuilt.found,
            next: reconstruct_success_guidance(rebuilt.matches_current),
        };
        Ok(())
    }

    fn apply_found(&mut self, found: FoundVault, network: Network) {
        self.network_mainnet = matches!(network, Network::Mainnet);
        self.status = format!(
            "Launcher 0x{} from {}. Paste the recovery phrase and Look up vault again.",
            hex::encode(found.launcher_id),
            found.launcher_source
        );
        self.step = AppStep::Found {
            vault_input: self.vault_address.trim().to_string(),
            found,
        };
    }

    fn apply_fallback(&mut self, gap: chia_vault_recover::LookupGap, network: Network) {
        self.network_mainnet = matches!(network, Network::Mainnet);
        let launcher = match gap.known_launcher() {
            Some(known) => format!(" launcher 0x{} ({}).", hex::encode(known.id), known.source),
            None => String::new(),
        };
        self.status = format!(
            "Lookup needs a self-send or vault-config.{launcher} {}",
            gap.headline()
        );
        self.step = AppStep::NeedFallback(gap);
    }

    fn run_lookup(&mut self) {
        let result = (|| {
            let vault = self.vault_address.trim();
            if vault.is_empty() {
                return Err(chia_vault_recover::Error::msg(
                    "enter the vault Receive address (xch1… / txch1…) first",
                ));
            }
            let mnemonic = {
                let trimmed = self.recovery_mnemonic.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed)
                }
            };
            if let (Some(found), Some(mnemonic)) = (self.cached_found(vault).cloned(), mnemonic) {
                let (_, network) = self.chain_client()?;
                let rebuilt = reconstruct(&found, mnemonic, self.clawback()?)?;
                self.apply_reconstructed(rebuilt, network)?;
                return Ok(());
            }

            let (client, network) = client_for_vault(vault, self.network(), &self.backend())?;
            let report = runtime().block_on(workflow::lookup(&client, vault))?;
            match report {
                LookupReport::Found(found) => {
                    if let Some(mnemonic) = mnemonic {
                        let rebuilt = reconstruct(&found, mnemonic, self.clawback()?)?;
                        self.apply_reconstructed(rebuilt, network)?;
                    } else {
                        self.apply_found(found, network);
                    }
                }
                LookupReport::NeedFallback(gap) => {
                    self.apply_fallback(gap, network);
                }
            }
            Ok::<(), chia_vault_recover::Error>(())
        })();
        if let Err(e) = result {
            self.step = AppStep::NeedLookup;
            self.status = format!("Lookup error: {e}");
        }
    }

    fn load_existing_config(&mut self) {
        let result = (|| {
            let config = VaultConfig::load(&self.config_path)?;
            self.step = AppStep::ReadyFromFile {
                next: LOADED_CONFIG_GUIDANCE.to_string(),
            };
            self.status = format!(
                "Loaded {}. launcher {}",
                self.config_path, config.launcher_id
            );
            Ok::<(), chia_vault_recover::Error>(())
        })();
        if let Err(e) = result {
            self.step = AppStep::NeedLookup;
            self.status = format!("Load config error: {e}");
        }
    }

    fn run_inspect(&mut self) {
        let result = (|| {
            let config = VaultConfig::load(&self.config_path)?;
            let post = if self.post_recovery_path.is_empty() {
                None
            } else {
                Some(VaultConfig::load(&self.post_recovery_path)?)
            };
            let (client, _) = self.chain_client()?;
            let report = runtime().block_on(workflow::inspect(&client, &config, post.as_ref()))?;
            self.set_ready_next(report.guidance.clone());
            self.status = match report.phase {
                VaultPhase::InRecovery => format!("Phase: InRecovery — {}", report.guidance),
                phase => format!("Phase: {phase:?}"),
            };
            Ok::<(), chia_vault_recover::Error>(())
        })();
        if let Err(e) = result {
            self.status = format!("Inspect error: {e}");
        }
    }

    fn run_start(&mut self) {
        let result = (|| {
            let config = VaultConfig::load(&self.config_path)?;
            let out = if self.post_recovery_path.is_empty() {
                PathBuf::from("post-recovery-vault-config.json")
            } else {
                PathBuf::from(&self.post_recovery_path)
            };
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
            let (client, network) = self.chain_client()?;
            let start = runtime().block_on(workflow::start(
                &client,
                StartWorkflow {
                    config: &config,
                    recovery_mnemonic: self.recovery_mnemonic.trim(),
                    new_custody_mnemonic: self.new_custody_mnemonic.trim(),
                    new_recovery_mnemonic: new_recovery,
                    new_clawback_timelock: None,
                    new_word_count: word_count,
                    network,
                    out_config: &out,
                },
            ))?;
            self.post_recovery_path = out.display().to_string();
            self.generated_recovery_mnemonic = start.generated_recovery_mnemonic.clone();
            self.status = format!(
                "Start pushed. Wait {}s then Finish. Config: {}",
                start.clawback_timelock,
                out.display()
            );
            self.set_ready_next(AFTER_START_GUIDANCE.to_string());
            Ok::<(), chia_vault_recover::Error>(())
        })();
        if let Err(e) = result {
            self.status = format!("Start error: {e}");
        }
    }

    fn run_finish(&mut self) {
        let result = (|| {
            let config = VaultConfig::load(&self.config_path)?;
            let post = VaultConfig::load(&self.post_recovery_path)?;
            let (client, network) = self.chain_client()?;
            let handle = runtime().block_on(workflow::finish(&client, &config, &post, network))?;
            self.status =
                format!("Finish pushed ({handle}). Vault custody is now the new BLS key.");
            Ok::<(), chia_vault_recover::Error>(())
        })();
        if let Err(e) = result {
            self.status = format!("Finish error: {e}");
        }
    }
}
