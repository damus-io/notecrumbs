use nostr_sdk::nips::nip19;
use std::array::TryFromSliceError;
use std::fmt;
use tokio::sync::broadcast::error::RecvError;

#[derive(Debug)]
pub enum Error {
    Nip19(nip19::Error),
    Http(hyper::http::Error),
    Nostrdb(nostrdb::Error),
    NostrClient(nostr_sdk::client::Error),
    Recv(RecvError),
    NotFound,
    InvalidNip19,
    SliceErr,
}

impl From<RecvError> for Error {
    fn from(err: RecvError) -> Self {
        Error::Recv(err)
    }
}

impl From<TryFromSliceError> for Error {
    fn from(_: TryFromSliceError) -> Self {
        Error::SliceErr
    }
}

impl From<nostr_sdk::client::Error> for Error {
    fn from(err: nostr_sdk::client::Error) -> Self {
        Error::NostrClient(err)
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
            Error::NostrClient(e) => write!(f, "Nostr client error: {}", e),
            Error::NotFound => write!(f, "Not found"),
            Error::Recv(e) => write!(f, "Recieve error: {}", e),
            Error::InvalidNip19 => write!(f, "Invalid nip19 object"),
            Error::SliceErr => write!(f, "Array slice error"),
        }
    }
}

impl std::error::Error for Error {}
