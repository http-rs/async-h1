use std::error::Error;
use std::fmt;

/// Errors when handling incoming requests.
#[derive(Debug)]
pub enum HttpError {
    UnexpectedContentLengthHeader,
}

impl fmt::Display for HttpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HttpError::UnexpectedContentLengthHeader => write!(f, "{}", self.description()),
        }
    }
}

impl Error for HttpError {
    fn description(&self) -> &str {
        match self {
            HttpError::UnexpectedContentLengthHeader => "unexpected content-length header",
        }
    }

    fn cause(&self) -> Option<&dyn Error> {
        None
    }
}
