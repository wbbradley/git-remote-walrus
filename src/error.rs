use std::fmt;

#[derive(Debug)]
#[allow(dead_code)]
pub enum Error {
    Io(std::io::Error),
    Yaml(serde_yaml::Error),
    Protocol(String),
    Storage(String),
    Git(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "IO error: {}", e),
            Error::Yaml(e) => write!(f, "YAML error: {}", e),
            Error::Protocol(msg) => write!(f, "Protocol error: {}", msg),
            Error::Storage(msg) => write!(f, "Storage error: {}", msg),
            Error::Git(msg) => write!(f, "Git error: {}", msg),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

impl From<serde_yaml::Error> for Error {
    fn from(e: serde_yaml::Error) -> Self {
        Error::Yaml(e)
    }
}
