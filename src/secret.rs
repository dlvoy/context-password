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
    /// rather than hiding behind an implicit `.clone()` — used when
    /// delivering a cached item's password while the cache itself must
    /// keep its own copy.
    pub fn clone_secret(&self) -> Self {
        Self(Zeroizing::new(self.0.to_string()))
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Secret(***)")
    }
}

#[cfg(test)]
mod tests {
    /// Regression guard for plan §8: nothing that can hold a password gets
    /// a `Debug` derive, since a stray `dbg!` or an over-eager log call on
    /// one of these types is the realistic way a vault leaks. `Secret`
    /// itself has the hand-written, redacting `Debug` impl above; this
    /// checks the *other* place passwords live.
    #[test]
    fn no_debug_derive_on_types_holding_passwords() {
        let files = [
            ("src/secret.rs", include_str!("secret.rs")),
            ("src/bw/model.rs", include_str!("bw/model.rs")),
        ];
        for (path, src) in files {
            // Only the non-test portion — `include_str!` embeds this test's
            // own source too, which mentions "derive(" and "Debug" as
            // string literals, not as an actual derive.
            let code = src.split("#[cfg(test)]").next().unwrap_or(src);
            let offending_line = code
                .lines()
                .find(|line| line.contains("derive(") && line.contains("Debug"));
            assert!(
                offending_line.is_none(),
                "{path} must not derive Debug on anything that can hold a password, found: {:?}",
                offending_line
            );
        }
    }
}
