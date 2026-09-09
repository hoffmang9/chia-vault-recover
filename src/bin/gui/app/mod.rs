//! Wizard app state: Look up → Start → Wait/Finish.

mod actions;
mod screens;

use std::path::PathBuf;
use std::sync::OnceLock;

use chia_vault_recover::cache::{CachedLookup, LookupCache};
use chia_vault_recover::chain::ChainClient;
use chia_vault_recover::locate::client_for_vault;
use chia_vault_recover::network::{Backend, Network};
use chia_vault_recover::{LookupGap, app_dir};
use eframe::egui::{self, RichText};

use crate::session::GuiSession;
use crate::theme;

fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("tokio runtime")
    })
}

const LOOKUP_SUBTITLE: &str =
    "Paste the Cloud Wallet Receive address. This check does not need the recovery phrase.";
const START_SUBTITLE: &str =
    "Enter the recovery phrase and a new custody mnemonic, then start delayed recovery.";
const WAIT_SUBTITLE: &str =
    "Wait for the clawback window, then finish. You can close the app and come back.";
const DONE_SUBTITLE: &str = "Recovery finished. Custody is now the new BLS key.";

/// Rail steps shown at the top of the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RailStep {
    Lookup,
    Start,
    Finish,
}

/// Exclusive wizard phase. Waiting owns the on-disk session; no parallel `has_config` flag.
#[derive(Debug, Clone)]
enum Phase {
    Lookup,
    Fallback(LookupGap),
    Start,
    Wait(GuiSession),
    Done,
}

pub struct App {
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
    status_is_error: bool,
    generated_recovery_mnemonic: Option<String>,
    phase: Phase,
    /// Last inspect / wait guidance (collapsible Details).
    detail: String,
    cache: LookupCache,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    pub fn new() -> Self {
        let cache = LookupCache::open();
        let (config_path, post_recovery_path) = default_config_paths();
        let mut app = Self {
            vault_address: String::new(),
            config_path,
            post_recovery_path,
            clawback_secs: String::new(),
            recovery_mnemonic: String::new(),
            new_custody_mnemonic: String::new(),
            new_recovery_mnemonic: String::new(),
            generate_12_words: false,
            network_mainnet: true,
            full_node_url: String::new(),
            status: String::new(),
            status_is_error: false,
            generated_recovery_mnemonic: None,
            phase: Phase::Lookup,
            detail: String::new(),
            cache,
        };
        app.resume_from_disk();
        app
    }

    /// No cache → Lookup; started session → Wait; cache only → Start.
    fn resume_from_disk(&mut self) {
        if let Some(session) = GuiSession::load().filter(|s| s.is_started())
            && session_files_ready(&session)
            && session_matches_cache(&self.cache, &session.receive_address)
        {
            self.apply_session_fields(&session);
            self.set_ok(
                "Resumed an in-progress recovery. Wait for the clawback window, then Finish.",
            );
            self.phase = Phase::Wait(session);
            return;
        }

        if let Some(entry) = self.cache.current().cloned() {
            self.vault_address = entry.receive_address.clone();
            if let Some(secs) = entry.clawback.secs() {
                self.clawback_secs = secs.to_string();
            }
            self.network_mainnet = matches!(entry.network, Network::Mainnet);
            self.phase = Phase::Start;
            self.set_ok(format!(
                "Loaded a saved lookup. Chain search was skipped. Cache: {}.",
                self.cache.path().display()
            ));
        }
    }

    fn apply_session_fields(&mut self, session: &GuiSession) {
        self.vault_address = session.receive_address.clone();
        self.config_path = session.config_path.clone();
        self.post_recovery_path = session.post_recovery_path.clone();
        if let Some(secs) = session.clawback_secs {
            self.clawback_secs = secs.to_string();
        }
        if let Some(entry) = self.cache.matching(&self.vault_address) {
            self.network_mainnet = matches!(entry.network, Network::Mainnet);
        }
    }

    fn set_ok(&mut self, message: impl Into<String>) {
        self.status = message.into();
        self.status_is_error = false;
    }

    fn set_err(&mut self, message: impl Into<String>) {
        self.status = message.into();
        self.status_is_error = true;
    }

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

    fn cached_vault(&self) -> Option<&CachedLookup> {
        self.cache.matching(&self.vault_address)
    }

    fn config_on_disk(&self) -> bool {
        let path = self.config_path.trim();
        !path.is_empty() && PathBuf::from(path).is_file()
    }

    fn can_start(&self) -> bool {
        self.cached_vault().is_some() || self.config_on_disk()
    }

    fn can_inspect(&self) -> bool {
        self.config_on_disk()
    }

    fn waiting_session(&self) -> Option<&GuiSession> {
        match &self.phase {
            Phase::Wait(session) => Some(session),
            _ => None,
        }
    }

    fn rail_step(&self) -> RailStep {
        match self.phase {
            Phase::Lookup | Phase::Fallback(_) => RailStep::Lookup,
            Phase::Start => RailStep::Start,
            Phase::Wait(_) | Phase::Done => RailStep::Finish,
        }
    }

    fn subtitle(&self) -> &'static str {
        match self.phase {
            Phase::Lookup | Phase::Fallback(_) => LOOKUP_SUBTITLE,
            Phase::Start => START_SUBTITLE,
            Phase::Wait(_) => WAIT_SUBTITLE,
            Phase::Done => DONE_SUBTITLE,
        }
    }

    fn clear_session_disk(&mut self) {
        GuiSession::clear();
    }

    fn reset_to_lookup(&mut self) {
        GuiSession::clear();
        self.vault_address.clear();
        self.clawback_secs.clear();
        self.recovery_mnemonic.clear();
        self.new_custody_mnemonic.clear();
        self.new_recovery_mnemonic.clear();
        self.generated_recovery_mnemonic = None;
        self.detail.clear();
        let (config_path, post_recovery_path) = default_config_paths();
        self.config_path = config_path;
        self.post_recovery_path = post_recovery_path;
        self.phase = Phase::Lookup;
        self.set_ok("Enter a Receive address to look up a vault.");
    }

    fn begin_wait(&mut self, clawback_secs: u64) {
        let session = GuiSession {
            receive_address: self.vault_address.trim().to_string(),
            config_path: self.config_path.clone(),
            post_recovery_path: self.post_recovery_path.clone(),
            clawback_secs: Some(clawback_secs),
            started_at: Some(GuiSession::now_unix()),
        };
        if let Err(e) = session.save() {
            self.set_ok(format!(
                "{}. Warning: could not save GUI session for relaunch: {e}",
                self.status
            ));
        }
        self.phase = Phase::Wait(session);
    }

    fn parsed_clawback(&self) -> chia_vault_recover::error::Result<Option<u64>> {
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

    fn ensure_parent_dir(path: &std::path::Path) -> chia_vault_recover::error::Result<()> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        Ok(())
    }

    fn resolve_config_path(&self) -> PathBuf {
        let trimmed = self.config_path.trim();
        if trimmed.is_empty() {
            app_dir().join("vault-config.json")
        } else {
            PathBuf::from(trimmed)
        }
    }

    fn resolve_post_path(&self) -> PathBuf {
        let trimmed = self.post_recovery_path.trim();
        if trimmed.is_empty() {
            app_dir().join("post-recovery-vault-config.json")
        } else {
            PathBuf::from(trimmed)
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.vertical(|ui| {
                ui.heading("Chia Vault Recover");
                ui.label(RichText::new(self.subtitle()).weak());
                ui.add_space(6.0);
                self.draw_step_rail(ui);
                ui.add_space(8.0);

                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        self.draw_current_screen(ui);
                    });

                ui.add_space(8.0);
                if !self.status.is_empty() {
                    theme::status_frame(ui, self.status_is_error).show(ui, |ui| {
                        ui.label(&self.status);
                    });
                }
            });
        });
    }
}

fn default_config_paths() -> (String, String) {
    let dir = app_dir();
    (
        dir.join("vault-config.json").display().to_string(),
        dir.join("post-recovery-vault-config.json")
            .display()
            .to_string(),
    )
}

fn session_files_ready(session: &GuiSession) -> bool {
    PathBuf::from(&session.config_path).is_file()
        && PathBuf::from(&session.post_recovery_path).is_file()
}

/// Resume Wait when the cache is empty or names the same receive address.
fn session_matches_cache(cache: &LookupCache, address: &str) -> bool {
    cache.current().is_none() || cache.matching(address).is_some()
}
