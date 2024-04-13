use quinn::{ApplicationClose, ConnectError, ConnectionError, ReadToEndError, WriteError};
use std::io;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum AppError {
    #[error("connection error: {0}")]
    ConnectionError(#[from] ConnectionError),
    #[error("connect error: {0}")]
    ConnectError(#[from] ConnectError),
    #[error("read error: {0}")]
    ReadToEndError(#[from] ReadToEndError),
    #[error("write error: {0}")]
    WriteError(#[from] WriteError),
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("bincode error: {0}")]
    Bincode(#[from] bincode::Error),
}

pub type AppResult<T> = Result<T, AppError>;

pub fn is_already_open_or_locally_closed_error(e: &AppError) -> bool {
    if let AppError::ConnectionError(e) = e {
        is_already_open_or_locally_closed_reason(e)
    } else {
        false
    }
}

pub fn is_already_open_or_locally_closed_reason(e: &ConnectionError) -> bool {
    if let ConnectionError::ApplicationClosed(ApplicationClose { error_code, .. }) = e {
        if error_code == &1u8.into() {
            return true;
        }
    }
    e == &ConnectionError::LocallyClosed
}
