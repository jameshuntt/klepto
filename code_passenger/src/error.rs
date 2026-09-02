use thiserror::Error;

#[derive(Debug, Error)]
pub enum PassengerError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

        #[error("serde_json error: {0}")]
    SerdeJsonError(#[from] serde_json::Error),


        #[error("toml error: {0}")]
    TomlError(#[from] toml::de::Error),


    #[error("invalid Cargo.toml: {0}")]
    Toml(String),

    #[error("missing [package].name in Cargo.toml")]
    MissingPackageName,

    #[error("path error: {0}")]
    Path(String),

    #[error("unsupported: {0}")]
    Unsupported(String),

    #[error("changes needed (run annotate --write)")]
    ChangesNeeded,
}

pub type Result<T> = std::result::Result<T, PassengerError>;

impl From<klepto::KleptoError> for PassengerError {
    fn from(e: klepto::KleptoError) -> Self {
        PassengerError::Path(format!("klepto: {e}")) // pick your variant
    }
}
