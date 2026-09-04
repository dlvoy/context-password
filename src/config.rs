//! On-disk configuration. Lives at `%APPDATA%\context-password\config.toml`.
//!
//! The master password is never part of this struct and never written to
//! disk — it exists only in memory for the lifetime of an unlock.

use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::vault::Provider;

/// Bounds for `max_visible_items`, enforced both by the Settings widget and
/// defensively by `Config::effective_max_visible`.
pub const MIN_VISIBLE_ITEMS: u32 = 3;
pub const MAX_VISIBLE_ITEMS: u32 = 10;

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
    /// Which credential backend to talk to. Defaults to `Bitwarden`, so a
    /// config file written before this field existed keeps working
    /// unchanged (the struct-level `#[serde(default)]` above is what makes
    /// that automatic).
    pub provider: Provider,
    /// Empty means "autodetect on PATH / known install locations".
    pub bw_path: String,
    /// Path to the `.kdbx` file, when `provider` is `KeePass`. Empty means
    /// "not chosen yet" — unlocking fails with a clear "pick one in
    /// Settings" message rather than a raw file-not-found error.
    pub keepass_path: String,
    /// The `app://` prefix used both to filter vault items and as the `bw
    /// list --search` prefilter term. Shared by both providers — it's the
    /// same tag grammar either way (see `vault::tag`).
    pub uri_prefix: String,
    /// How long to wait after regaining foreground before typing starts.
    pub type_settle_ms: u32,
    /// How many tagged items the popup shows at once. Not read directly —
    /// use `effective_max_visible` — since a hand-edited config file could
    /// carry a value outside the range the Settings widget enforces.
    pub max_visible_items: u32,
    /// Whether to run `bw lock` on quit. Off by default: it would invalidate
    /// session keys the user may be relying on in other terminals.
    pub lock_on_exit: bool,
    pub debug_log: bool,
}

/// The out-of-the-box hotkey spec, in `global_hotkey::hotkey::HotKey::from_str`
/// form. macOS gets `Cmd` instead of `Ctrl` — the platform-conventional
/// modifier for a global shortcut, and one Windows doesn't have.
#[cfg(windows)]
const DEFAULT_HOTKEY: &str = "Ctrl+Alt+KeyV";
#[cfg(target_os = "macos")]
const DEFAULT_HOTKEY: &str = "Cmd+Shift+KeyV";

impl Default for Config {
    fn default() -> Self {
        Self {
            hotkey: DEFAULT_HOTKEY.to_string(),
            unlock_mode: UnlockMode::Delayed,
            unlock_delay_secs: 20,
            autostart: false,
            provider: Provider::Bitwarden,
            bw_path: String::new(),
            keepass_path: String::new(),
            uri_prefix: "app://context-password".to_string(),
            type_settle_ms: 30,
            max_visible_items: 4,
            lock_on_exit: false,
            debug_log: false,
        }
    }
}

impl Config {
    /// The number of items the popup should actually show, clamped to
    /// `MIN_VISIBLE_ITEMS..=MAX_VISIBLE_ITEMS` regardless of what's on disk
    /// — a hand-edited config shouldn't be able to produce a popup sized
    /// for an out-of-range count.
    pub fn effective_max_visible(&self) -> usize {
        self.max_visible_items.clamp(MIN_VISIBLE_ITEMS, MAX_VISIBLE_ITEMS) as usize
    }

    /// `Err` with a user-facing reason when the current provider isn't
    /// usable yet — today that's only "KeePass selected but no database
    /// chosen". Used both by Settings validation (refuse to save) and the
    /// worker's own guard (`keepass::unlock_and_list` returns the same
    /// message independently, since a config file could be hand-edited
    /// into this state without ever going through Settings).
    pub fn provider_ready(&self) -> Result<(), String> {
        if self.provider == Provider::KeePass && self.keepass_path.trim().is_empty() {
            return Err("No KeePass database selected — pick one in Settings.".to_string());
        }
        Ok(())
    }

    #[cfg(windows)]
    pub fn config_path() -> io::Result<PathBuf> {
        let appdata = std::env::var_os("APPDATA")
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "%APPDATA% is not set"))?;
        Ok(PathBuf::from(appdata)
            .join("context-password")
            .join("config.toml"))
    }

    #[cfg(target_os = "macos")]
    pub fn config_path() -> io::Result<PathBuf> {
        let home = std::env::var_os("HOME")
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "$HOME is not set"))?;
        Ok(PathBuf::from(home)
            .join("Library")
            .join("Application Support")
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
        // The single most important compatibility property of the KeePass
        // provider work: a config file from before `provider` existed must
        // land on Bitwarden, not on some arbitrary default.
        assert_eq!(cfg.provider, Provider::Bitwarden);
    }

    #[test]
    fn provider_serializes_as_expected_toml_strings() {
        // `#[serde(rename_all = "snake_case")]` alone would write `KeePass`
        // as "kee_pass" — an ugly, permanent wart in every user's
        // config.toml. Guards the explicit `#[serde(rename = "keepass")]`
        // override on that variant.
        let bw = Config { provider: Provider::Bitwarden, ..Config::default() };
        assert!(toml::to_string(&bw).unwrap().contains(r#"provider = "bitwarden""#));

        let kp = Config { provider: Provider::KeePass, ..Config::default() };
        assert!(toml::to_string(&kp).unwrap().contains(r#"provider = "keepass""#));

        let parsed: Config = toml::from_str(r#"provider = "keepass""#).unwrap();
        assert_eq!(parsed.provider, Provider::KeePass);
    }

    #[test]
    fn provider_ready_requires_a_keepass_path_only_for_the_keepass_provider() {
        let bw = Config { provider: Provider::Bitwarden, keepass_path: String::new(), ..Config::default() };
        assert!(bw.provider_ready().is_ok());

        let kp_unset = Config { provider: Provider::KeePass, keepass_path: String::new(), ..Config::default() };
        assert!(kp_unset.provider_ready().is_err());

        let kp_set = Config {
            provider: Provider::KeePass,
            keepass_path: "/tmp/whatever.kdbx".to_string(),
            ..Config::default()
        };
        assert!(kp_set.provider_ready().is_ok());
    }

    #[test]
    fn effective_max_visible_clamps_an_out_of_range_value() {
        let cfg = Config { max_visible_items: 1, ..Config::default() };
        assert_eq!(cfg.effective_max_visible(), MIN_VISIBLE_ITEMS as usize);
        let cfg = Config { max_visible_items: 99, ..Config::default() };
        assert_eq!(cfg.effective_max_visible(), MAX_VISIBLE_ITEMS as usize);
        let cfg = Config { max_visible_items: 5, ..Config::default() };
        assert_eq!(cfg.effective_max_visible(), 5);
    }
}
