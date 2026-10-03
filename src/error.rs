use std::fmt;
use std::path::{Path, PathBuf};

/// Errors that can occur when converting between GRPs and PNGs.
#[derive(Debug)]
pub enum Error {
    /// Reading or writing a file failed.
    Io(std::io::Error),
    /// Reading or writing a PNG or palette failed.
    Png(palpngrs::Error),
    /// The input does not look like a valid GRP, or its data is inconsistent.
    InvalidGrp(String),
    /// The input cannot be represented in the GRP format being written.
    CannotEncode(String),
    /// The arguments are invalid for the given input.
    InvalidArgument(String),
    /// Another error, annotated with the file it relates to.
    InFile { path: PathBuf, source: Box<Error> },
}

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e)  => write!(f, "{e}"),
            Self::Png(e) => write!(f, "{e}"),
            Self::InvalidGrp(msg)      => write!(f, "invalid GRP: {msg}"),
            Self::CannotEncode(msg)    => write!(f, "cannot create GRP: {msg}"),
            Self::InvalidArgument(msg) => f.write_str(msg),
            Self::InFile { path, source } => write!(f, "{}: {source}", path.display()),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            // Io and Png display their inner error's message, so skip
            // straight to its source to avoid reporting it twice.
            Self::Io(e)  => e.source(),
            Self::Png(e) => e.source(),
            Self::InFile { source, .. } => Some(source.as_ref()),
            _ => None,
        }
    }
}

impl From<std::io::Error>  for Error { fn from(e: std::io::Error)  -> Self { Self::Io(e)  } }
impl From<palpngrs::Error> for Error { fn from(e: palpngrs::Error) -> Self { Self::Png(e) } }

impl Error {
    /// Returns the underlying error, skipping any file annotations.
    pub fn root(&self) -> &Error {
        match self {
            Error::InFile { source, .. } => source.root(),
            other => other,
        }
    }
}

/// Annotates errors with the file they relate to.
pub trait InFile<T> {
    fn in_file(self, path: impl AsRef<Path>) -> Result<T>;
}

impl<T, E: Into<Error>> InFile<T> for std::result::Result<T, E> {
    fn in_file(self, path: impl AsRef<Path>) -> Result<T> {
        self.map_err(|e| Error::InFile {
            path:   path.as_ref().to_path_buf(),
            source: Box::new(e.into()),
        })
    }
}
