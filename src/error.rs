use nostr_sdk::nips::nip19;
use std::array::TryFromSliceError;
use std::fmt;
use tokio::sync::broadcast::error::RecvError;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    Nip19(nip19::Error),
    Http(hyper::http::Error),
    Hyper(hyper::Error),
    Nostrdb(nostrdb::Error),
    NostrClient(nostr_sdk::client::Error),
    Recv(RecvError),
    Io(std::io::Error),
    Generic(String),
    Timeout(tokio::time::error::Elapsed),
    Image(image::error::ImageError),
    Secp(nostr_sdk::secp256k1::Error),
    InvalidUri,
    NotFound,
    /// Profile picture is too big
    #[allow(dead_code)]
    TooBig,
    InvalidNip19,
    #[allow(dead_code)]
    InvalidProfilePic,
    CantRender,
    SliceErr,
}

impl From<image::error::ImageError> for Error {
    fn from(err: image::error::ImageError) -> Self {
        Error::Image(err)
    }
}

impl From<http::uri::InvalidUri> for Error {
    fn from(_err: http::uri::InvalidUri) -> Self {
        Error::InvalidUri
    }
}

impl From<nostr_sdk::secp256k1::Error> for Error {
    fn from(err: nostr_sdk::secp256k1::Error) -> Self {
        Error::Secp(err)
    }
}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        Error::Io(err)
    }
}

impl From<String> for Error {
    fn from(err: String) -> Self {
        Error::Generic(err)
    }
}

impl From<RecvError> for Error {
    fn from(err: RecvError) -> Self {
        Error::Recv(err)
    }
}

impl From<tokio::time::error::Elapsed> for Error {
    fn from(err: tokio::time::error::Elapsed) -> Self {
        Error::Timeout(err)
    }
}

impl From<hyper::Error> for Error {
    fn from(err: hyper::Error) -> Self {
        Error::Hyper(err)
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
            Error::TooBig => write!(f, "Profile picture is too big"),
            Error::InvalidProfilePic => write!(f, "Profile picture is corrupt"),
            Error::CantRender => write!(f, "Error rendering"),
            Error::Image(err) => write!(f, "Image error: {}", err),
            Error::Timeout(elapsed) => write!(f, "Timeout error: {}", elapsed),
            Error::InvalidUri => write!(f, "Invalid url"),
            Error::Hyper(err) => write!(f, "Hyper error: {}", err),
            Error::Generic(err) => write!(f, "Generic error: {}", err),
            Error::Io(err) => write!(f, "Io error: {}", err),
            Error::Secp(err) => write!(f, "Signature error: {}", err),
        }
    }
}

impl std::error::Error for Error {}
