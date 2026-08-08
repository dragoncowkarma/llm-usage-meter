//! Phase 2: Abstract Platform Layer.
//!
//! Anything that differs between macOS and Windows lives here. Providers depend
//! on these abstractions only, so porting means adding an impl in this module
//! rather than editing every provider.

pub mod paths;

pub use paths::{first_existing, PlatformPaths};

/// Everything a provider needs to know about the host, resolved once at startup
/// and shared immutably across refreshes.
pub struct HostEnv {
    pub paths: Box<dyn PlatformPaths>,
}

impl HostEnv {
    pub fn detect() -> Self {
        Self {
            paths: paths::current(),
        }
    }

    #[cfg(test)]
    pub fn with_paths(paths: Box<dyn PlatformPaths>) -> Self {
        Self { paths }
    }
}
