//! Wizard screen drawing.

use chia_vault_recover::discover::ClawbackGuess;
use chia_vault_recover::guidance::{CLAWBACK_SECS_HELP, OPTIONAL_CONFIRM_HELP, fallback_guidance};
use eframe::egui::{self, RichText};

use crate::theme::{self, DANGER, primary_button, secondary_button};

use super::{App, Phase, RailStep};

impl App {
    pub(super) fn draw_current_screen(&mut self, ui: &mut egui::Ui) {
        match &self.phase {
            Phase::Lookup => self.draw_lookup(ui),
            Phase::Fallback(_) => self.draw_fallback(ui),
            Phase::Start => self.draw_start(ui),
            Phase::Wait(_) => self.draw_wait_finish(ui),
            Phase::Done => self.draw_done(ui),
        }
    }

    pub(super) fn draw_step_rail(&self, ui: &mut egui::Ui) {
        let current = self.rail_step();
        ui.horizontal(|ui| {
            for (step, label) in [
                (RailStep::Lookup, "Look up"),
                (RailStep::Start, "Start"),
                (RailStep::Finish, "Finish"),
            ] {
                let active = step == current;
                let done = matches!(
                    (current, step),
                    (RailStep::Start, RailStep::Lookup)
                        | (RailStep::Finish, RailStep::Lookup | RailStep::Start)
                );
                let text = if active {
                    RichText::new(label).strong().color(theme::CHIA_GREEN)
                } else if done {
                    RichText::new(label).weak()
                } else {
                    RichText::new(label)
                };
                ui.label(text);
                if step != RailStep::Finish {
                    ui.label(RichText::new("→").weak());
                }
            }
        });
        ui.separator();
    }

    fn draw_vault_summary(&self, ui: &mut egui::Ui) {
        theme::card_frame(ui).show(ui, |ui| {
            ui.strong("Saved vault");
            let address = self.vault_address.trim();
            if address.is_empty() {
                ui.label("No receive address yet.");
                return;
            }
            ui.horizontal(|ui| {
                ui.label("Address:");
                ui.monospace(truncate_middle(address, 20, 12));
            });
            ui.label(format!(
                "Network: {}",
                if self.network_mainnet {
                    "mainnet"
                } else {
                    "testnet11"
                }
            ));
            if let Some(entry) = self.cached_vault() {
                let launcher = hex::encode(entry.found.launcher_id);
                ui.horizontal(|ui| {
                    ui.label("Launcher:");
                    ui.monospace(format!("0x{}…", &launcher[..8.min(launcher.len())]));
                });
                ui.label(clawback_label(entry.clawback));
            } else if let Some(secs) = self.waiting_session().and_then(|s| s.clawback_secs) {
                ui.label(format!("Clawback: {secs}s"));
            }
        });
    }

    fn path_row(ui: &mut egui::Ui, path: &mut String) {
        ui.horizontal(|ui| {
            ui.text_edit_singleline(path);
            if secondary_button(ui, "Browse…").clicked()
                && let Some(picked) = rfd::FileDialog::new().pick_file()
            {
                *path = picked.display().to_string();
            }
        });
    }

    fn draw_lookup(&mut self, ui: &mut egui::Ui) {
        theme::card_frame(ui).show(ui, |ui| {
            ui.strong("Receive address");
            ui.add(
                egui::TextEdit::singleline(&mut self.vault_address)
                    .desired_width(f32::INFINITY)
                    .hint_text("xch1… or txch1…"),
            );
            ui.horizontal(|ui| {
                ui.label("Network:");
                if ui
                    .selectable_label(self.network_mainnet, "Mainnet")
                    .clicked()
                {
                    self.network_mainnet = true;
                }
                if ui
                    .selectable_label(!self.network_mainnet, "Testnet11")
                    .clicked()
                {
                    self.network_mainnet = false;
                }
                ui.label(RichText::new("(xch1 / txch1 overrides)").small().weak());
            });

            ui.add_space(4.0);
            if primary_button(ui, "Look up vault").clicked() {
                self.run_lookup();
            }
        });

        ui.add_space(10.0);
        ui.collapsing("Advanced", |ui| {
            ui.label("Full node URL (optional; empty = coinset):");
            ui.text_edit_singleline(&mut self.full_node_url);
            ui.add_space(6.0);
            ui.label("Already have a vault-config JSON?");
            ui.label(
                RichText::new(
                    "Only needed if lookup says the chain does not yet show this vault’s layout.",
                )
                .small()
                .weak(),
            );
            Self::path_row(ui, &mut self.config_path);
            if secondary_button(ui, "Load config").clicked() {
                self.load_existing_config();
            }
        });
    }

    fn draw_fallback(&mut self, ui: &mut egui::Ui) {
        // Copy is &'static / owned String — no need to hold or clone LookupGap across draws.
        let Some((headline, detail, guidance)) = (match &self.phase {
            Phase::Fallback(gap) => Some((gap.headline(), gap.detail(), fallback_guidance(gap))),
            _ => None,
        }) else {
            return;
        };

        theme::card_frame(ui).show(ui, |ui| {
            ui.colored_label(DANGER, headline);
            ui.label(detail);
            ui.add_space(6.0);
            ui.label("Preferred: send any amount from the vault back to the same Receive address, wait for confirmation, then look up again.");
            ui.add_space(4.0);
            if primary_button(ui, "Look up again").clicked() {
                self.run_lookup();
            }
            ui.add_space(4.0);
            if secondary_button(ui, "Load vault-config JSON…").clicked()
                && let Some(path) = rfd::FileDialog::new().pick_file()
            {
                self.config_path = path.display().to_string();
                self.load_existing_config();
            }
        });
        ui.collapsing("Details", |ui| {
            ui.label(guidance);
        });
    }

    fn draw_start(&mut self, ui: &mut egui::Ui) {
        self.draw_vault_summary(ui);
        ui.add_space(10.0);

        if !self.can_start() {
            theme::card_frame(ui).show(ui, |ui| {
                ui.label("Look up a vault first (or load a vault-config JSON).");
                if secondary_button(ui, "Back to look up").clicked() {
                    self.phase = Phase::Lookup;
                }
            });
            return;
        }

        theme::card_frame(ui).show(ui, |ui| {
            if self.cached_vault().is_some() {
                ui.label(
                    RichText::new(
                        "Optional: check clawback now, or enter it when you start. The recovery phrase is never written to disk.",
                    )
                    .small()
                    .weak(),
                );
            }

            ui.label("Cloud Wallet recovery passphrase");
            ui.add(
                egui::TextEdit::multiline(&mut self.recovery_mnemonic)
                    .desired_rows(2)
                    .desired_width(f32::INFINITY),
            );

            ui.horizontal(|ui| {
                ui.label("Clawback seconds (optional):");
                ui.add(
                    egui::TextEdit::singleline(&mut self.clawback_secs)
                        .desired_width(120.0)
                        .hint_text("e.g. 43200"),
                );
            });
            if self.cached_vault().is_some()
                && secondary_button(ui, "Check clawback now").clicked()
            {
                self.run_confirm_clawback();
            }

            ui.collapsing("Clawback help", |ui| {
                ui.label(OPTIONAL_CONFIRM_HELP);
                ui.label(CLAWBACK_SECS_HELP);
            });

            ui.add_space(4.0);
            ui.label("New custody mnemonic (required)");
            ui.add(
                egui::TextEdit::multiline(&mut self.new_custody_mnemonic)
                    .desired_rows(2)
                    .desired_width(f32::INFINITY),
            );
            ui.label("New recovery mnemonic (optional — leave empty to auto-generate)");
            ui.add(
                egui::TextEdit::multiline(&mut self.new_recovery_mnemonic)
                    .desired_rows(2)
                    .desired_width(f32::INFINITY),
            );
            ui.checkbox(
                &mut self.generate_12_words,
                "Generate 12-word mnemonics (default 24)",
            );

            ui.collapsing("Config paths", |ui| {
                ui.label("Vault config (written at Start, or loaded JSON):");
                Self::path_row(ui, &mut self.config_path);
                ui.label("Post-recovery config:");
                Self::path_row(ui, &mut self.post_recovery_path);
            });

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if primary_button(ui, "Start recovery").clicked() {
                    self.run_start();
                }
                if self.can_inspect() && secondary_button(ui, "Inspect").clicked() {
                    self.run_inspect();
                }
            });
        });

        ui.add_space(8.0);
        if secondary_button(ui, "Look up a different vault").clicked() {
            self.reset_to_lookup();
        }
    }

    fn draw_wait_finish(&mut self, ui: &mut egui::Ui) {
        self.draw_vault_summary(ui);
        ui.add_space(10.0);

        theme::card_frame(ui).show(ui, |ui| {
            if let Some(remaining) = self
                .waiting_session()
                .and_then(|s| s.clawback_remaining_secs())
            {
                if remaining > 0 {
                    ui.label(format!(
                        "Clawback window: about {} remaining.",
                        format_duration(remaining as u64)
                    ));
                    ui.label(
                        RichText::new("Old custody can still cancel recovery until this ends.")
                            .small()
                            .weak(),
                    );
                } else {
                    ui.colored_label(
                        theme::CHIA_GREEN,
                        "Clawback window has elapsed. You can Finish.",
                    );
                }
            } else if let Ok(Some(secs)) = self.parsed_clawback() {
                ui.label(format!(
                    "Clawback window: {secs}s (started time unknown — wait that long from Start, then Finish)."
                ));
            } else {
                ui.label("Wait for the clawback window, then Finish recovery.");
            }

            if let Some(words) = self.generated_recovery_mnemonic.clone() {
                ui.add_space(8.0);
                ui.colored_label(
                    DANGER,
                    "SAVE THIS NEW RECOVERY MNEMONIC (not written to config):",
                );
                ui.monospace(&words);
                if secondary_button(ui, "Copy recovery mnemonic").clicked() {
                    ui.ctx().copy_text(words);
                    self.set_ok("Copied recovery mnemonic to clipboard.");
                }
            }

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if primary_button(ui, "Finish recovery").clicked() {
                    self.run_finish();
                }
                if self.can_inspect() && secondary_button(ui, "Inspect").clicked() {
                    self.run_inspect();
                }
            });

            if !self.detail.is_empty() {
                ui.collapsing("Details", |ui| {
                    ui.label(&self.detail);
                });
            }
        });

        ui.add_space(8.0);
        if secondary_button(ui, "Look up a different vault").clicked() {
            self.reset_to_lookup();
        }
    }

    fn draw_done(&mut self, ui: &mut egui::Ui) {
        theme::card_frame(ui).show(ui, |ui| {
            ui.colored_label(theme::CHIA_GREEN, "Recovery complete.");
            ui.label(
                "Vault custody is now the new BLS key. Keep the new recovery mnemonic safe if one was generated.",
            );
            if secondary_button(ui, "Look up another vault").clicked() {
                self.reset_to_lookup();
            }
        });
    }
}

fn clawback_label(guess: ClawbackGuess) -> String {
    match guess {
        ClawbackGuess::Unknown => "Clawback: not set".into(),
        ClawbackGuess::Hint(secs) => format!("Clawback: {secs}s (hint)"),
        ClawbackGuess::Known(secs) => format!("Clawback: {secs}s (verified)"),
    }
}

fn truncate_middle(s: &str, head: usize, tail: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= head + tail + 1 {
        return s.to_string();
    }
    let left: String = chars.iter().take(head).collect();
    let right: String = chars
        .iter()
        .rev()
        .take(tail)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    format!("{left}…{right}")
}

fn format_duration(secs: u64) -> String {
    let hours = secs / 3600;
    let mins = (secs % 3600) / 60;
    let s = secs % 60;
    if hours > 0 {
        format!("{hours}h {mins}m")
    } else if mins > 0 {
        format!("{mins}m {s}s")
    } else {
        format!("{s}s")
    }
}
