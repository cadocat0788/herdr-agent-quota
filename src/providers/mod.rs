pub mod agy;
pub mod claude;
pub mod codex;
pub mod deepseek;
pub mod grok;
pub mod opencode_go;
pub mod statusline;
pub mod zcode_glm;

use thiserror::Error;

#[derive(Debug, PartialEq, Eq, Error)]
pub enum ProviderError {
    #[error("provider credentials are unavailable")]
    MissingCredentials,
    #[error("provider quota is unavailable: {0}")]
    Unavailable(String),
    #[error("provider response is not supported: {0}")]
    UnsupportedResponse(String),
    #[error("provider request failed: {0}")]
    Request(String),
}
