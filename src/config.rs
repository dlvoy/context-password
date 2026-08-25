//! On-disk configuration. Lives at `%APPDATA%\context-password\config.toml`.
//!
//! The master password is never part of this struct and never written to
//! disk — it exists only in memory for the lifetime of an unlock.

use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnlockMode {
    /// Prompt for the master password `unlock_delay_secs` after startup.
    #[default]
    Delayed,
    /// Prompt only when the popup is first opened.
    Lazy,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// `global_hotkey::hotkey::HotKey::from_str` form, e.g. "Ctrl+Alt+KeyV".
    pub hotkey: String,
    pub unlock_mode: UnlockMode,
    pub unlock_delay_secs: u32,
    pub autostart: bool,
    /// Empty means "autodetect on PATH / known install locations".
    pub bw_path: String,
    /// The `app://` prefix used both to filter vault items and as the `bw
    /// list --search` prefilter term.
    pub uri_prefix: String,
    /// How long to wait after regaining foreground before typing starts.
    pub type_settle_ms: u32,
    pub max_visible_items: u32,
    /// Whether to run `bw lock` on quit. Off by default: it would invalidate
    /// session keys the user may be relying on in other terminals.
    pub lock_on_exit: bool,
    pub debug_log: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            hotkey: "Ctrl+Alt+KeyV".to_string(),
            unlock_mode: UnlockMode::Delayed,
            unlock_delay_secs: 20,
            autostart: false,
            bw_path: String::new(),
            uri_prefix: "app://context-password".to_string(),
            type_settle_ms: 30,
            max_visible_items: 12,
            lock_on_exit: false,
            debug_log: false,
        }
    }
}

impl Config {
    pub fn config_path() -> io::Result<PathBuf> {
        let appdata = std::env::var_os("APPDATA")
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "%APPDATA% is not set"))?;
        Ok(PathBuf::from(appdata)
            .join("context-password")
            .join("config.toml"))
    }

    /// Loads the config from disk, creating and persisting the default if no
    /// file exists yet. A corrupt file is reported rather than silently
    /// overwritten, so the user never loses a hand-edited config to a typo.
    pub fn load() -> io::Result<Self> {
        let path = Self::config_path()?;
        match fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{}: {e}", path.display()),
                )
            }),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let cfg = Config::default();
                cfg.save()?;
                Ok(cfg)
            }
            Err(e) => Err(e),
        }
    }

    /// Writes the config atomically: a temp file is written first and renamed
    /// over the target, so a crash mid-write never leaves a truncated config.
    pub fn save(&self) -> io::Result<()> {
        let path = Self::config_path()?;
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let text = toml::to_string_pretty(self)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        let tmp = path.with_extension("toml.tmp");
        fs::write(&tmp, text)?;
        fs::rename(&tmp, &path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_default_through_toml() {
        let cfg = Config::default();
        let text = toml::to_string_pretty(&cfg).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(cfg, back);
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        // #[serde(default)] on the struct means a partial file (e.g. from an
        // older version that had fewer fields) still loads instead of erroring.
        let cfg: Config = toml::from_str(r#"hotkey = "Ctrl+Alt+KeyB""#).unwrap();
        assert_eq!(cfg.hotkey, "Ctrl+Alt+KeyB");
        assert_eq!(cfg.unlock_delay_secs, Config::default().unlock_delay_secs);
    }
}
