//! Error types for the verifier service

use thiserror::Error;

pub type VerifierResult<T> = std::result::Result<T, VerifierError>;

/// Errors that can occur during verification
#[derive(Error, Debug, Clone)]
pub enum VerifierError {
    #[error("Insufficient results: got {received}, need at least {required}")]
    InsufficientResults { received: usize, required: usize },

    #[error("No consensus reached: {0} different results")]
    NoConsensus(usize),

    #[error("Task not found: {0}")]
    TaskNotFound(String),

    #[error("Provider already submitted result: {0}")]
    DuplicateProvider(String),

    #[error("Invalid result format: {0}")]
    InvalidResult(String),

    #[error("Timeout waiting for results: {0}")]
    Timeout(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

