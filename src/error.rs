use nostr_sdk::nips::nip19;
use std::array::TryFromSliceError;
use std::error::Error as StdError;
use std::fmt;

#[derive(Debug)]
pub enum Error {
    Nip19(nip19::Error),
    Http(hyper::http::Error),
    Nostrdb(nostrdb::Error),
    SliceErr,
}

impl From<TryFromSliceError> for Error {
    fn from(_: TryFromSliceError) -> Self {
        Error::SliceErr
    }
}

impl From<nip19::Error> for Error {
    fn from(err: nip19::Error) -> Self {
        Error::Nip19(err)
    }
}

impl From<hyper::http::Error> for Error {
    fn from(err: hyper::http::Error) -> Self {
        Error::Http(err)
    }
}

impl From<nostrdb::Error> for Error {
    fn from(err: nostrdb::Error) -> Self {
        Error::Nostrdb(err)
    }
}

// Implementing `Display`
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Nip19(e) => write!(f, "Nip19 error: {}", e),
            Error::Http(e) => write!(f, "HTTP error: {}", e),
            Error::Nostrdb(e) => write!(f, "Nostrdb error: {}", e),
            Error::SliceErr => write!(f, "Array slice error"),
        }
    }
}

// Implementing `StdError`
impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Error::Nip19(e) => Some(e),
            Error::Http(e) => Some(e),
            Error::Nostrdb(e) => Some(e),
            Error::SliceErr => None,
        }
    }
}
