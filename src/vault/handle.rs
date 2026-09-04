//! `VaultHandle` — owns the currently-active vault worker (`bw::spawn` or
//! `keepass::spawn`) and knows how to tear it down and respawn a different
//! one when Settings changes the provider (or that provider's own config:
//! `bw_path`/`uri_prefix` for Bitwarden, `keepass_path` for KeePass).
//!
//! Before this existed, the worker was spawned once at startup with its
//! config captured by value into the thread closure, and the returned
//! `Sender` sat in a plain struct field with no swap mechanism — a
//! provider switch in Settings had no way to take effect without an app
//! restart. This is that missing mechanism.

use std::sync::mpsc::Sender;

use crate::config::Config;
use crate::msg::{Msg, VaultCmd};
use crate::{bw, keepass};

use super::{Provider, Waker};

/// The subset of `Config` that determines what the active worker was
/// spawned with — compared on every `needs_respawn` call so a change to
/// any of these (not just `provider` itself) triggers a respawn. Pointing
/// KeePass at a different `.kdbx` file with the provider unchanged must
/// still drop the old file's decrypted contents, for instance.
#[derive(Clone, PartialEq, Eq)]
struct SpawnKey {
    provider: Provider,
    bw_path: String,
    uri_prefix: String,
    keepass_path: String,
}

impl SpawnKey {
    fn from_config(cfg: &Config) -> Self {
        Self {
            provider: cfg.provider,
            bw_path: cfg.bw_path.clone(),
            uri_prefix: cfg.uri_prefix.clone(),
            keepass_path: cfg.keepass_path.clone(),
        }
    }
}

/// No `Debug` derive — this fronts a channel that carries `Secret`s
/// (`VaultCmd::Unlock`), and the discipline elsewhere in this codebase
/// (plan §8) is that nothing on that path gets a derive that could print
/// it, even indirectly.
pub struct VaultHandle {
    cmd_tx: Sender<VaultCmd>,
    generation: u64,
    spawned_with: SpawnKey,
    results_tx: Sender<Msg>,
    wake: Waker,
}

impl VaultHandle {
    /// Spawns the worker for `cfg.provider`, at generation `0`.
    pub fn spawn(cfg: &Config, results_tx: Sender<Msg>, wake: Waker) -> Self {
        let generation = 0;
        let cmd_tx = spawn_worker(cfg, generation, results_tx.clone(), wake.clone());
        Self {
            cmd_tx,
            generation,
            spawned_with: SpawnKey::from_config(cfg),
            results_tx,
            wake,
        }
    }

    /// True if the *live* worker no longer matches `cfg` — the provider
    /// changed, or the provider's own config did.
    pub fn needs_respawn(&self, cfg: &Config) -> bool {
        self.spawned_with != SpawnKey::from_config(cfg)
    }

    /// Tears down the current worker and spawns a fresh one, bumping
    /// `generation`. Teardown is just letting `self.cmd_tx` be replaced —
    /// dropping the last `Sender` to a worker's channel ends its
    /// `recv()` loop, which drops (and, for a retained key, zeroizes) that
    /// worker's locals. No kill flag, no join: if the old worker is
    /// mid-call, it finishes, sends its (now-stale-generation) result, sees
    /// the channel closed, and exits — bounded by that call's own timeout,
    /// never blocking this call.
    pub fn respawn(&mut self, cfg: &Config) {
        self.generation += 1;
        self.spawned_with = SpawnKey::from_config(cfg);
        self.cmd_tx = spawn_worker(cfg, self.generation, self.results_tx.clone(), self.wake.clone());
    }

    pub fn send(&self, cmd: VaultCmd) {
        let _ = self.cmd_tx.send(cmd);
    }

    pub fn provider(&self) -> Provider {
        self.spawned_with.provider
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Whether a `Msg::Vault { generation, .. }` belongs to the *live*
    /// worker — a stale generation means the result is from a worker
    /// already torn down by a provider switch, and must be dropped rather
    /// than applied.
    pub fn accepts(&self, generation: u64) -> bool {
        generation == self.generation
    }
}

fn spawn_worker(cfg: &Config, generation: u64, results_tx: Sender<Msg>, wake: Waker) -> Sender<VaultCmd> {
    match cfg.provider {
        Provider::Bitwarden => {
            bw::spawn(cfg.bw_path.clone(), cfg.uri_prefix.clone(), generation, results_tx, wake)
        }
        Provider::KeePass => {
            keepass::spawn(cfg.keepass_path.clone(), cfg.uri_prefix.clone(), generation, results_tx, wake)
        }
    }
}
