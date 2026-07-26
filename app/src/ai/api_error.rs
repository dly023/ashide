use crate::ai::byop_readiness::BlockedByopReadinessError;
use serde::{Deserialize, Serialize};
use warp_core::errors::{AnyhowErrorExt, ErrorExt};
use warp_core::register_error;

#[derive(thiserror::Error, Debug, Serialize, Deserialize)]
#[error("{error}")]
pub struct ClientError {
    pub error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_url: Option<String>,
}

#[derive(thiserror::Error, Debug)]
pub enum DeserializationError {
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Transport(reqwest::Error),
}

#[derive(thiserror::Error, Debug)]
pub enum AIApiError {
    #[error("Request failed due to lack of AI quota.")]
    QuotaLimit,

    #[error("Ashide is currently overloaded. Please try again later.")]
    ServerOverloaded,

    #[error("Internal error occurred at transport layer.")]
    Transport(#[source] reqwest::Error),

    #[error("Failed to deserialize API response.")]
    Deserialization(#[source] DeserializationError),

    #[error("No context found on context search.")]
    NoContextFound,

    #[error("Failed with status code {0}: {1}")]
    ErrorStatus(http::StatusCode, String),

    #[error(transparent)]
    Other(#[from] anyhow::Error),

    #[error("Got error when streaming {stream_type}: {source:#}")]
    Stream {
        stream_type: &'static str,
        #[source]
        source: anyhow::Error,
    },
}

impl From<http_client::ResponseError> for AIApiError {
    fn from(err: http_client::ResponseError) -> Self {
        Self::from_response_error(err.source)
    }
}

impl From<reqwest::Error> for AIApiError {
    fn from(err: reqwest::Error) -> Self {
        Self::from_transport_error(err)
    }
}

impl From<serde_json::Error> for AIApiError {
    fn from(err: serde_json::Error) -> Self {
        AIApiError::Deserialization(err.into())
    }
}

impl AIApiError {
    fn from_response_error(err: reqwest::Error) -> Self {
        if err.status() == Some(http::StatusCode::TOO_MANY_REQUESTS) {
            return AIApiError::ServerOverloaded;
        }

        Self::from_transport_error(err)
    }

    fn from_transport_error(err: reqwest::Error) -> Self {
        if err.is_timeout() {
            return AIApiError::Transport(err);
        }
        if err.is_decode() {
            #[cfg(not(target_family = "wasm"))]
            {
                use std::error::Error as _;
                let mut source = err.source();
                while let Some(underlying) = source {
                    if underlying.is::<hyper::Error>() {
                        return AIApiError::Transport(err);
                    }

                    source = underlying.source();
                }
            }

            return AIApiError::Deserialization(DeserializationError::Transport(err));
        }

        AIApiError::Transport(err)
    }

    pub fn is_retryable(&self) -> bool {
        fn is_retryable_status(status: http::StatusCode) -> bool {
            !status.is_client_error()
                || status == http::StatusCode::REQUEST_TIMEOUT
                || status == http::StatusCode::TOO_MANY_REQUESTS
        }

        match self {
            AIApiError::ErrorStatus(status, _) => is_retryable_status(*status),
            AIApiError::Transport(e) => {
                if let Some(status) = e.status() {
                    return is_retryable_status(status);
                }
                true
            }
            AIApiError::QuotaLimit
            | AIApiError::ServerOverloaded
            | AIApiError::Deserialization(_)
            | AIApiError::NoContextFound
            | AIApiError::Stream { .. } => true,
            AIApiError::Other(error) => error.downcast_ref::<BlockedByopReadinessError>().is_none(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::byop_readiness::{BlockedByopReadinessError, ReadinessCategory};

    #[test]
    fn byop_blocked_readiness_error_is_not_retryable() {
        let error = AIApiError::Other(
            BlockedByopReadinessError::new(ReadinessCategory::MissingResultWithoutRepairSource)
                .into(),
        );

        assert!(!error.is_retryable());
    }
}

impl ErrorExt for AIApiError {
    fn is_actionable(&self) -> bool {
        match self {
            AIApiError::Deserialization(_) => true,
            AIApiError::Transport(error) => error.is_actionable(),
            AIApiError::Other(error) => error.is_actionable(),
            AIApiError::Stream { source, .. } => source.is_actionable(),
            AIApiError::ErrorStatus(_, _) => self.is_retryable(),
            AIApiError::QuotaLimit | AIApiError::ServerOverloaded | AIApiError::NoContextFound => {
                false
            }
        }
    }
}

register_error!(AIApiError);
