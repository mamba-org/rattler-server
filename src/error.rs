//! Contains the errors that the API can return when trying to solve an environment

use rattler_solve::SolveError;
use serde::{Serialize, Serializer};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("internal error")]
    Internal(#[from] anyhow::Error),
    #[error("validation error: {0}")]
    Validation(#[from] ValidationError),
    #[error("solve error: {0}")]
    Solver(#[from] SolveError),
}

#[derive(Debug, Error)]
pub enum ValidationError {
    #[error("invalid match specs")]
    MatchSpecs(ParseErrors),
    #[error("invalid virtual package")]
    VirtualPackage(ParseError),
    #[error("invalid channels")]
    Channels(ParseErrors),
    #[error("invalid platform")]
    Platform(ParseError),
}

impl Serialize for ValidationError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            ValidationError::MatchSpecs(errors) | ValidationError::Channels(errors) => {
                errors.serialize(serializer)
            }
            ValidationError::VirtualPackage(error) | ValidationError::Platform(error) => {
                error.serialize(serializer)
            }
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ParseError {
    pub input: String,
    pub error: String,
}

#[derive(Debug, Serialize)]
pub struct ParseErrors(pub Vec<ParseError>);
