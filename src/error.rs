//! Contains the errors that the API can return when trying to solve an environment

use crate::dto::SolveEnvironmentErr;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use rattler_repodata_gateway::fetch::FetchRepoDataError;
use rattler_solve::SolveError;
use reqwest::Url;
use serde::{Serialize, Serializer};
use thiserror::Error;
use tracing::{event, Level};

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("internal error")]
    Internal(#[from] anyhow::Error),
    #[error("validation error: {0}")]
    Validation(#[from] ValidationError),
    #[error("error fetching repodata.json from {}", .0.to_string())]
    FetchRepoDataJson(Url, #[source] FetchRepoDataError),
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

fn rewrite_error(api_error: ApiError) -> ApiError {
    match api_error {
        ApiError::Solver(error @ SolveError::UnsupportedOperations(_)) => {
            ApiError::Internal(error.into())
        }
        _ => api_error,
    }
}

pub fn response_from_error(api_error: ApiError) -> Response {
    let api_error = rewrite_error(api_error);
    match api_error {
        ApiError::Internal(e) => {
            event!(
                Level::ERROR,
                "Internal server error: {} (caused by {})",
                e.to_string(),
                e.source()
                    .map(|e2| e2.to_string())
                    .unwrap_or("unknown source".to_string())
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(SolveEnvironmentErr::<()> {
                    error_kind: "internal".to_string(),
                    message: None,
                    additional_info: None,
                }),
            )
                .into_response()
        }
        ApiError::FetchRepoDataJson(url, e) => {
            event!(
                Level::WARN,
                "Error fetching repodata.json: {}",
                e.to_string()
            );
            (
                StatusCode::BAD_REQUEST,
                Json(SolveEnvironmentErr {
                    error_kind: "http".to_string(),
                    message: Some("unable to retrieve repodata.json".to_string()),
                    additional_info: Some(format!("url: {url}")),
                }),
            )
                .into_response()
        }
        ApiError::Validation(e) => (
            StatusCode::BAD_REQUEST,
            Json(SolveEnvironmentErr {
                error_kind: "validation".to_string(),
                message: Some(e.to_string()),
                additional_info: Some(e),
            }),
        )
            .into_response(),
        ApiError::Solver(SolveError::UnsupportedOperations(_)) => unreachable!(),
        ApiError::Solver(SolveError::Unsolvable(e)) => (
            StatusCode::CONFLICT,
            Json(SolveEnvironmentErr {
                error_kind: "solver".to_string(),
                message: Some("no solution found for the specified dependencies".to_string()),
                additional_info: Some(e),
            }),
        )
            .into_response(),
        ApiError::Solver(SolveError::ParseMatchSpecError(e)) => (
            StatusCode::BAD_REQUEST,
            Json(SolveEnvironmentErr {
                error_kind: "validation".to_string(),
                message: Some("invalid match spec".to_string()),
                additional_info: Some(e.to_string()),
            }),
        )
            .into_response(),
        ApiError::Solver(SolveError::Cancelled) => (
            StatusCode::BAD_REQUEST,
            Json(SolveEnvironmentErr::<String> {
                error_kind: "validation".to_string(),
                message: Some("solver process cancelled".to_string()),
                additional_info: None,
            }),
        )
            .into_response(),
    }
}

#[tokio::test]
async fn test_unsupported_operations_is_mapped_to_response() {
    let error = ApiError::Solver(SolveError::UnsupportedOperations(vec!["foo".to_string()]));
    let response = response_from_error(error);

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}
