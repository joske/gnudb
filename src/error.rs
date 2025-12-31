#[derive(thiserror::Error, Debug)]
pub enum GnuDbError {
    #[error("Connection Error: {0}")]
    ConnectionError(String),
    #[error("Protocol Error: {0}")]
    ProtocolError(String),
}

impl From<std::io::Error> for GnuDbError {
    fn from(err: std::io::Error) -> Self {
        GnuDbError::ConnectionError(err.to_string())
    }
}

impl From<ureq::Error> for GnuDbError {
    fn from(err: ureq::Error) -> Self {
        match err {
            ureq::Error::StatusCode(code) => {
                GnuDbError::ProtocolError(format!("HTTP status {code}"))
            }
            _ => GnuDbError::ConnectionError(err.to_string()),
        }
    }
}
