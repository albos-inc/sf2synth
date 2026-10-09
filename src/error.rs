use std::fmt;

/// Errors returned while loading a SoundFont or a MIDI file.
#[derive(Debug)]
pub enum Error {
    /// Reading the input failed.
    Io(std::io::Error),
    /// The data is not a usable SF2 file.
    InvalidSoundFont(String),
    /// The SFZ file (or every sample it plays) is not usable.
    InvalidSfz(String),
    /// The data is not a usable Standard MIDI File.
    InvalidMidiFile(String),
    /// A setting is out of its range.
    InvalidSettings(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(error) => write!(f, "I/O error: {error}"),
            Error::InvalidSoundFont(message) => write!(f, "invalid SF2 file: {message}"),
            Error::InvalidSfz(message) => write!(f, "invalid SFZ file: {message}"),
            Error::InvalidMidiFile(message) => write!(f, "invalid MIDI file: {message}"),
            Error::InvalidSettings(message) => write!(f, "invalid settings: {message}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Error::Io(error)
    }
}

/// `Result` with this crate's [`Error`].
pub type Result<T> = std::result::Result<T, Error>;
