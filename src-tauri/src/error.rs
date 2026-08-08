use thiserror::Error;

/// `NotInstalled` and `Other` are part of the provider-facing error vocabulary
/// (a third-party provider returns them); the bundled providers happen not to.
#[allow(dead_code)]
#[derive(Debug, Error)]
pub enum MeterError {
    #[error("{0} is not installed for this user")]
    NotInstalled(&'static str),

    #[error("io error at {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("could not parse {path}: {message}")]
    Parse { path: String, message: String },

    #[error("{0}")]
    Other(String),
}

impl MeterError {
    pub fn io(path: impl AsRef<std::path::Path>, source: std::io::Error) -> Self {
        MeterError::Io {
            path: path.as_ref().display().to_string(),
            source,
        }
    }

    pub fn parse(path: impl AsRef<std::path::Path>, message: impl Into<String>) -> Self {
        MeterError::Parse {
            path: path.as_ref().display().to_string(),
            message: message.into(),
        }
    }
}

pub type Result<T> = std::result::Result<T, MeterError>;

/// Tauri commands must return something serialisable.
impl serde::Serialize for MeterError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}
