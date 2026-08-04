//! Minimum-viable egui GUI for vault recovery.

use std::path::PathBuf;
use std::sync::OnceLock;

use chia_vault_recover::chain::ChainClient;
use chia_vault_recover::config::VaultConfig;
use chia_vault_recover::keys::MnemonicWordCount;
use chia_vault_recover::network::{Backend, Network};
use chia_vault_recover::recovery::VaultPhase;
use chia_vault_recover::workflow::{self, StartWorkflow};
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
            .with_inner_size([720.0, 640.0])
            .with_title("Chia Vault Recover"),
        ..Default::default()
    };
    eframe::run_native(
        "Chia Vault Recover",
        options,
        Box::new(|_cc| Ok(Box::new(App::default()))),
    )
}

struct App {
    config_path: String,
    post_recovery_path: String,
    recovery_mnemonic: String,
    new_custody_mnemonic: String,
    new_recovery_mnemonic: String,
    generate_12_words: bool,
    network_mainnet: bool,
    full_node_url: String,
    status: String,
    generated_recovery_mnemonic: Option<String>,
    last_guidance: String,
}

impl Default for App {
    fn default() -> Self {
        Self {
            config_path: String::new(),
            post_recovery_path: String::new(),
            recovery_mnemonic: String::new(),
            new_custody_mnemonic: String::new(),
            new_recovery_mnemonic: String::new(),
            generate_12_words: false,
            network_mainnet: true,
            full_node_url: String::new(),
            status: String::new(),
            generated_recovery_mnemonic: None,
            last_guidance: String::new(),
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

    fn client(&self) -> ChainClient {
        ChainClient::new(self.network(), &self.backend())
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Chia Vault Recover");
            ui.label("Delayed recovery with a Cloud Wallet recovery passphrase.");
            ui.separator();

            ui.horizontal(|ui| {
                ui.label("Vault config:");
                ui.text_edit_singleline(&mut self.config_path);
                if ui.button("Browse…").clicked()
                    && let Some(path) = rfd::FileDialog::new().pick_file()
                {
                    self.config_path = path.display().to_string();
                }
            });

            ui.horizontal(|ui| {
                ui.label("Post-recovery config:");
                ui.text_edit_singleline(&mut self.post_recovery_path);
                if ui.button("Browse…").clicked()
                    && let Some(path) = rfd::FileDialog::new().pick_file()
                {
                    self.post_recovery_path = path.display().to_string();
                }
            });

            ui.checkbox(&mut self.network_mainnet, "Mainnet (unchecked = testnet11)");
            ui.horizontal(|ui| {
                ui.label("Full node URL (optional; empty = coinset):");
                ui.text_edit_singleline(&mut self.full_node_url);
            });
            ui.checkbox(
                &mut self.generate_12_words,
                "Generate 12-word mnemonics (default 24)",
            );

            ui.separator();
            ui.label("Recovery mnemonic (Cloud Wallet):");
            ui.add(
                egui::TextEdit::multiline(&mut self.recovery_mnemonic)
                    .desired_rows(2)
                    .desired_width(f32::INFINITY),
            );
            ui.label("New custody mnemonic:");
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

            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Inspect").clicked() {
                    self.run_inspect();
                }
                if ui.button("Start recovery").clicked() {
                    self.run_start();
                }
                if ui.button("Finish recovery").clicked() {
                    self.run_finish();
                }
            });

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

            if !self.last_guidance.is_empty() {
                ui.separator();
                ui.label(&self.last_guidance);
            }
            if !self.status.is_empty() {
                ui.separator();
                ui.label(&self.status);
            }
        });
    }
}

impl App {
    fn run_inspect(&mut self) {
        let result = (|| {
            let config = VaultConfig::load(&self.config_path)?;
            let post = if self.post_recovery_path.is_empty() {
                None
            } else {
                Some(VaultConfig::load(&self.post_recovery_path)?)
            };
            let client = self.client();
            let report = runtime().block_on(workflow::inspect(&client, &config, post.as_ref()))?;
            self.last_guidance = report.guidance.clone();
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
            let client = self.client();
            let network = self.network();
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
            self.last_guidance =
                "Vault entering RECOVERY. After the clawback period, click Finish recovery.".into();
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
            let client = self.client();
            let network = self.network();
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
