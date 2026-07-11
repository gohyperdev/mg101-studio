//! Błędy domenowe — port `PatchError`/`ProfileError` ze Swift (MG101Core).

use std::fmt;

/// Błąd operacji na rekordzie patcha.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchError {
    /// Rozmiar danych ≠ `recordSize`.
    InvalidSize { expected: usize, actual: usize },
    /// Offset poza rekordem.
    Offset(usize),
    /// Wartość bajtu poza 0..=255.
    Value(i64),
    /// BPM poza zakresem profilu.
    Bpm(i64),
    /// Nazwa dłuższa niż dozwolona (bajty UTF-8).
    NameTooLong(usize),
    /// Wartość parametru poza zakresem [min,max].
    ParameterRange {
        name: String,
        minimum: i64,
        maximum: i64,
        value: i64,
    },
    /// Zła liczba wartości parametrów względem modelu.
    ParameterCount { expected: usize, actual: usize },
    /// Nieznane pole globalne.
    UnknownField(String),
    /// Niepoprawny załącznik IR.
    Ir(String),
}

impl fmt::Display for PatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PatchError::InvalidSize { expected, actual } => {
                write!(f, "Expected {expected} bytes, received {actual}.")
            }
            PatchError::Offset(v) => write!(f, "Offset outside record: {v}."),
            PatchError::Value(v) => write!(f, "Byte value outside 0...255: {v}."),
            PatchError::Bpm(v) => write!(f, "BPM outside configured range: {v}."),
            PatchError::NameTooLong(m) => write!(f, "Patch name exceeds {m} UTF-8 bytes."),
            PatchError::ParameterRange {
                name,
                minimum,
                maximum,
                value,
            } => write!(f, "{name}={value} is outside {minimum}...{maximum}."),
            PatchError::ParameterCount { expected, actual } => {
                write!(
                    f,
                    "Expected {expected} parameter values, received {actual}."
                )
            }
            PatchError::UnknownField(n) => write!(f, "Unknown named field: {n}."),
            PatchError::Ir(m) => write!(f, "Invalid local IR: {m}"),
        }
    }
}

impl std::error::Error for PatchError {}

/// Błąd profilu/katalogu urządzenia.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileError {
    /// Brak zasobu.
    MissingResource(String),
    /// Nieznany blok.
    UnknownBlock(String),
    /// Nieznany model w bloku.
    UnknownModel(String, i64),
    /// Profil/katalog niepoprawny.
    Malformed(String),
}

impl fmt::Display for ProfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProfileError::MissingResource(n) => write!(f, "Missing bundled profile resource: {n}"),
            ProfileError::UnknownBlock(id) => write!(f, "Unknown block: {id}"),
            ProfileError::UnknownModel(b, id) => write!(f, "Unknown {b} model: {id}"),
            ProfileError::Malformed(m) => write!(f, "Malformed profile: {m}"),
        }
    }
}

impl std::error::Error for ProfileError {}
