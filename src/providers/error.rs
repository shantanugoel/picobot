use std::time::Duration;

#[derive(Debug, Clone, thiserror::Error)]
pub enum ProviderError {
    #[error("rate limited")]
    RateLimit { retry_after: Option<Duration> },
    #[error("transient provider error: {message}")]
    Transient { message: String },
    #[error("permanent provider error: {message}")]
    Permanent { message: String },
}

impl ProviderError {
    pub fn from_anyhow(err: anyhow::Error) -> Self {
        let message = err.to_string();
        let lower = message.to_ascii_lowercase();
        if lower.contains("rate limit") || lower.contains("429") {
            return ProviderError::RateLimit { retry_after: None };
        }
        if lower.contains("timeout")
            || lower.contains("timed out")
            || lower.contains("connection")
            || lower.contains("temporar")
            || lower.contains("unavailable")
            || lower.contains("503")
            || lower.contains("502")
            || lower.contains("504")
        {
            return ProviderError::Transient { message };
        }
        ProviderError::Permanent { message }
    }

    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            ProviderError::RateLimit { .. } | ProviderError::Transient { .. }
        )
    }

    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            ProviderError::RateLimit { retry_after } => *retry_after,
            _ => None,
        }
    }
}
