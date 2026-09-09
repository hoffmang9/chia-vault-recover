//! Public GUI session (no mnemonics) so Finish works after relaunch.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use chia_vault_recover::app_dir;
use chia_vault_recover::error::Result;

const SESSION_FILE: &str = "gui-session.json";

/// Paths and clawback timing for an in-progress recovery. Never stores mnemonics.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GuiSession {
    pub receive_address: String,
    pub config_path: String,
    pub post_recovery_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clawback_secs: Option<u64>,
    /// Unix seconds when Start recovery succeeded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<u64>,
}

impl GuiSession {
    pub fn default_path() -> PathBuf {
        app_dir().join(SESSION_FILE)
    }

    pub fn load() -> Option<Self> {
        Self::load_at(Self::default_path())
    }

    pub fn load_at(path: impl AsRef<Path>) -> Option<Self> {
        let text = fs::read_to_string(path).ok()?;
        serde_json::from_str(&text).ok()
    }

    pub fn save(&self) -> Result<()> {
        self.save_at(Self::default_path())
    }

    pub fn save_at(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    pub fn clear() {
        Self::clear_at(Self::default_path());
    }

    pub fn clear_at(path: impl AsRef<Path>) {
        let _ = fs::remove_file(path);
    }

    pub fn is_started(&self) -> bool {
        self.started_at.is_some()
            && !self.config_path.trim().is_empty()
            && !self.post_recovery_path.trim().is_empty()
    }

    pub fn now_unix() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    /// Seconds remaining until clawback ends, if Start time and clawback are known.
    pub fn clawback_remaining_secs(&self) -> Option<i64> {
        let started = self.started_at?;
        let secs = self.clawback_secs?;
        let now = Self::now_unix();
        let end = started.saturating_add(secs);
        Some(end as i64 - now as i64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "cvr-gui-session-{}-{}.json",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_file(&path);
        path
    }

    #[test]
    fn roundtrip_without_secrets() {
        let path = temp_path();
        let session = GuiSession {
            receive_address: "xch1abc".into(),
            config_path: "/tmp/vault-config.json".into(),
            post_recovery_path: "/tmp/post.json".into(),
            clawback_secs: Some(43_200),
            started_at: Some(1_700_000_000),
        };
        session.save_at(&path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(!text.contains("mnemonic"));
        let loaded = GuiSession::load_at(&path).unwrap();
        assert_eq!(loaded, session);
        assert!(loaded.is_started());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn clear_removes_file() {
        let path = temp_path();
        let session = GuiSession {
            receive_address: "xch1abc".into(),
            config_path: "a".into(),
            post_recovery_path: "b".into(),
            clawback_secs: None,
            started_at: None,
        };
        session.save_at(&path).unwrap();
        GuiSession::clear_at(&path);
        assert!(GuiSession::load_at(&path).is_none());
    }
}
