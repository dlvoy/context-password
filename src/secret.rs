//! A `String` wrapper that refuses to be printed, implicitly cloned, or
//! serialized — every place a master password, session key, or vault
//! password moves through the app should be visible at a glance in the
//! source (plan §8).

use zeroize::Zeroizing;

pub struct Secret(Zeroizing<String>);

impl Secret {
    pub fn new(value: String) -> Self {
        Self(Zeroizing::new(value))
    }

    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Explicit, greppable duplication. `Secret` deliberately has no
    /// `Clone` impl, so every copy of a secret is visible in the source
    /// rather than hiding behind an implicit `.clone()`. Unused until M5
    /// needs to duplicate a selected item's password for delivery.
    #[allow(dead_code)]
    pub fn clone_secret(&self) -> Self {
        Self(Zeroizing::new(self.0.to_string()))
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Secret(***)")
    }
}
