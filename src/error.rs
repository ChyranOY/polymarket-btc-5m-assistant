use thiserror::Error;

pub type Result<T> = std::result::Result<T, BotError>;

#[derive(Debug, Error)]
pub enum BotError {
    #[error("config: {0}")]
    Config(String),

    #[error("http: {0}")]
    Http(#[from] reqwest::Error),

    #[error("ws: {0}")]
    Ws(String),

    #[error("parse: {0}")]
    Parse(String),

    #[error("supabase: {status} {body}")]
    Supabase { status: u16, body: String },

    #[error("clob: {0}")]
    Clob(String),

    #[error("signing: {0}")]
    Signing(String),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("decimal: {0}")]
    Decimal(#[from] rust_decimal::Error),

    #[error("other: {0}")]
    Other(String),
}

impl BotError {
    pub fn cfg(msg: impl Into<String>) -> Self {
        Self::Config(msg.into())
    }
    pub fn parse(msg: impl Into<String>) -> Self {
        Self::Parse(msg.into())
    }
    pub fn other(msg: impl Into<String>) -> Self {
        Self::Other(msg.into())
    }
}
