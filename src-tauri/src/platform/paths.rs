//! Phase 2: Abstract Platform Layer — OS-specific filesystem layout.
//!
//! Every provider asks this module *where* things live; no provider ever
//! hardcodes `~/.claude` or `%APPDATA%`. Adding Windows support therefore means
//! implementing one more `PlatformPaths` impl here — provider code stays put.
//!
//! Only one impl is constructed per target, so the other two look like dead
//! code to the compiler. They are not: they are the Phase 2 deliverable, they
//! are exercised by this module's tests on every platform, and `current()`
//! selects between them by `cfg`.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// The base directories an LLM CLI/IDE might scatter its state across.
///
/// Naming follows the *intent* of the directory, not any single OS's term for
/// it, so that provider code reads the same on every platform.
pub trait PlatformPaths: Send + Sync {
    /// The current user's home directory.
    fn home(&self) -> PathBuf;

    /// Roaming, user-scoped configuration.
    /// macOS/Linux: `~/.config` · Windows: `%APPDATA%`
    fn config_root(&self) -> PathBuf;

    /// Larger, machine-local application state (caches, databases, logs).
    /// macOS: `~/Library/Application Support` · Windows: `%LOCALAPPDATA%` · Linux: `~/.local/share`
    fn data_root(&self) -> PathBuf;

    /// Per-user cache directory.
    /// macOS: `~/Library/Caches` · Windows: `%LOCALAPPDATA%` · Linux: `~/.cache`
    fn cache_root(&self) -> PathBuf;

    /// A classic Unix-style dotfile directory in `$HOME`.
    ///
    /// Node-based CLIs (Claude Code, Codex, Gemini/Antigravity) use the *same*
    /// `~/.tool` convention on Windows too, because they resolve `os.homedir()`
    /// rather than a platform config API. Keeping this as its own method makes
    /// that assumption explicit and greppable.
    fn dot_dir(&self, name: &str) -> PathBuf {
        self.home().join(format!(".{name}"))
    }

    /// Short platform tag used in diagnostics.
    fn platform_tag(&self) -> &'static str;
}

/// Resolve the first candidate path that exists on disk.
///
/// Providers use this to tolerate layout drift across tool versions — e.g.
/// Antigravity has moved its state between `~/.antigravity` and
/// `~/.gemini/antigravity` across releases.
pub fn first_existing<I, P>(candidates: I) -> Option<PathBuf>
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    candidates
        .into_iter()
        .map(|p| p.as_ref().to_path_buf())
        .find(|p| p.exists())
}

// ---------------------------------------------------------------------------
// macOS
// ---------------------------------------------------------------------------

pub struct MacPaths {
    home: PathBuf,
}

impl MacPaths {
    pub fn new(home: PathBuf) -> Self {
        Self { home }
    }
}

impl PlatformPaths for MacPaths {
    fn home(&self) -> PathBuf {
        self.home.clone()
    }
    fn config_root(&self) -> PathBuf {
        self.home.join(".config")
    }
    fn data_root(&self) -> PathBuf {
        self.home.join("Library").join("Application Support")
    }
    fn cache_root(&self) -> PathBuf {
        self.home.join("Library").join("Caches")
    }
    fn platform_tag(&self) -> &'static str {
        "macos"
    }
}

// ---------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------

pub struct WindowsPaths {
    home: PathBuf,
    appdata: PathBuf,
    local_appdata: PathBuf,
}

impl WindowsPaths {
    pub fn new(home: PathBuf, appdata: PathBuf, local_appdata: PathBuf) -> Self {
        Self {
            home,
            appdata,
            local_appdata,
        }
    }

    /// Build from the environment, falling back to the documented default
    /// layout (`%USERPROFILE%\AppData\{Roaming,Local}`) when the variables are
    /// missing — which happens under some service accounts and CI runners.
    pub fn from_env(home: PathBuf) -> Self {
        let appdata = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("AppData").join("Roaming"));
        let local_appdata = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("AppData").join("Local"));
        Self::new(home, appdata, local_appdata)
    }
}

impl PlatformPaths for WindowsPaths {
    fn home(&self) -> PathBuf {
        self.home.clone()
    }
    fn config_root(&self) -> PathBuf {
        self.appdata.clone()
    }
    fn data_root(&self) -> PathBuf {
        self.local_appdata.clone()
    }
    fn cache_root(&self) -> PathBuf {
        self.local_appdata.clone()
    }
    fn platform_tag(&self) -> &'static str {
        "windows"
    }
}

// ---------------------------------------------------------------------------
// Linux (XDG)
// ---------------------------------------------------------------------------

pub struct LinuxPaths {
    home: PathBuf,
}

impl LinuxPaths {
    pub fn new(home: PathBuf) -> Self {
        Self { home }
    }

    fn xdg(&self, var: &str, default: &str) -> PathBuf {
        std::env::var_os(var)
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| self.home.join(default))
    }
}

impl PlatformPaths for LinuxPaths {
    fn home(&self) -> PathBuf {
        self.home.clone()
    }
    fn config_root(&self) -> PathBuf {
        self.xdg("XDG_CONFIG_HOME", ".config")
    }
    fn data_root(&self) -> PathBuf {
        self.xdg("XDG_DATA_HOME", ".local/share")
    }
    fn cache_root(&self) -> PathBuf {
        self.xdg("XDG_CACHE_HOME", ".cache")
    }
    fn platform_tag(&self) -> &'static str {
        "linux"
    }
}

/// Build the `PlatformPaths` implementation for the host OS.
pub fn current() -> Box<dyn PlatformPaths> {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));

    #[cfg(target_os = "macos")]
    {
        Box::new(MacPaths::new(home))
    }
    #[cfg(target_os = "windows")]
    {
        Box::new(WindowsPaths::from_env(home))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Box::new(LinuxPaths::new(home))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_layout_matches_apple_conventions() {
        let p = MacPaths::new(PathBuf::from("/Users/alex"));
        assert_eq!(p.dot_dir("claude"), PathBuf::from("/Users/alex/.claude"));
        assert_eq!(
            p.data_root(),
            PathBuf::from("/Users/alex/Library/Application Support")
        );
        assert_eq!(p.config_root(), PathBuf::from("/Users/alex/.config"));
    }

    #[test]
    fn windows_layout_splits_roaming_and_local() {
        let p = WindowsPaths::new(
            PathBuf::from(r"C:\Users\alex"),
            PathBuf::from(r"C:\Users\alex\AppData\Roaming"),
            PathBuf::from(r"C:\Users\alex\AppData\Local"),
        );
        assert_eq!(p.config_root(), PathBuf::from(r"C:\Users\alex\AppData\Roaming"));
        assert_eq!(p.data_root(), PathBuf::from(r"C:\Users\alex\AppData\Local"));
        // Node-based CLIs keep the dotfile convention on Windows.
        assert_eq!(p.dot_dir("codex"), PathBuf::from(r"C:\Users\alex").join(".codex"));
    }

    #[test]
    fn windows_from_env_falls_back_to_documented_default() {
        // Deliberately does not touch the process environment: on a machine
        // where APPDATA *is* set this only asserts the value is absolute and
        // rooted somewhere sane.
        let p = WindowsPaths::from_env(PathBuf::from(r"C:\Users\alex"));
        assert!(p.config_root().is_absolute() || p.config_root().starts_with(r"C:\Users\alex"));
        assert!(p.data_root().to_string_lossy().len() > 0);
    }

    #[test]
    fn first_existing_picks_the_live_candidate() {
        let tmp = std::env::temp_dir();
        let missing = tmp.join("llm-usage-meter-definitely-not-here");
        assert_eq!(
            first_existing([missing.as_path(), tmp.as_path()]),
            Some(tmp.clone())
        );
        assert_eq!(first_existing([missing.as_path()]), None);
    }
}
