#[derive(thiserror::Error)]
pub enum GnuDbError {
    #[error("Connection Error: {0}")]
    ConnectionError(String),
    #[error("Protocol Error: {0}")]
    ProtocolError(String),
}

impl std::fmt::Debug for GnuDbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GnuDbError::ConnectionError(msg) => write!(f, "ConnectionError({})", msg),
            GnuDbError::ProtocolError(msg) => write!(f, "ProtocolError({})", msg),
        }
    }
}

impl From<std::io::Error> for GnuDbError {
    fn from(err: std::io::Error) -> Self {
        GnuDbError::ConnectionError(err.to_string())
    }
}
