use std::fmt;

/// A user-facing failure: a stable machine code plus an English message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppError {
    pub code: &'static str,
    pub message: String,
}

pub type Result<T> = std::result::Result<T, AppError>;

impl AppError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn usage(message: impl Into<String>) -> Self {
        Self::new("usage", message)
    }

    pub fn io(context: impl fmt::Display, err: std::io::Error) -> Self {
        Self::new("io", format!("{context}: {err}"))
    }

    pub fn exit_code(&self) -> i32 {
        if self.code == "usage" { 2 } else { 1 }
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
