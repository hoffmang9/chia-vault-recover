//! Wizard screen drawing.

use chia_vault_recover::guidance::{CLAWBACK_SECS_HELP, OPTIONAL_CONFIRM_HELP, fallback_guidance};
use eframe::egui::{self, RichText};

use crate::theme::{self, DANGER, secondary_button};

use super::{App, Phase};

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

    fn draw_lookup(&mut self, ui: &mut egui::Ui) {
        theme::card_frame(ui).show(ui, |ui| {
            ui.strong("Receive address");
            ui.add(
                egui::TextEdit::singleline(&mut self.vault_address)
                    .desired_width(f32::INFINITY)
                    .hint_text("xch1… or txch1…"),
            );
            self.network_toggle(ui);

            ui.add_space(4.0);
            if self.primary_action(ui, "Look up vault", true) {
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
            if self.primary_action(ui, "Look up again", true) {
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

        let can_start = self.can_start();
        theme::card_frame(ui).show(ui, |ui| {
            if !can_start {
                ui.label(
                    RichText::new(
                        "Look up a vault or load a vault-config JSON before starting recovery.",
                    )
                    .weak(),
                );
                ui.add_space(6.0);
            } else if self.cached_vault().is_some() {
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
                if self.primary_action(ui, "Start recovery", can_start) {
                    self.run_start();
                }
                if self.config_on_disk() && secondary_button(ui, "Inspect").clicked() {
                    self.run_inspect();
                }
            });
        });

        ui.add_space(8.0);
        self.draw_different_vault_button(ui);
    }

    fn draw_wait_finish(&mut self, ui: &mut egui::Ui) {
        self.draw_vault_summary(ui);
        ui.add_space(10.0);

        // Snapshot session fields so we can mutate self while drawing.
        let countdown = self.waiting_session().cloned();
        let can_inspect = self.config_on_disk();

        theme::card_frame(ui).show(ui, |ui| {
            if let Some(session) = &countdown {
                self.draw_clawback_countdown(ui, session);
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
                if self.primary_action(ui, "Finish recovery", true) {
                    self.run_finish();
                }
                if can_inspect && secondary_button(ui, "Inspect").clicked() {
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
        self.draw_different_vault_button(ui);
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
