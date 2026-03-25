use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::Request;
use axum::http::{HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use serde::{Deserialize, Serialize};
use tracing::{info_span, warn, Instrument};
use uuid::Uuid;

pub const CORRELATION_ID_HEADER: &str = "X-Correlation-Id";
pub const REQUEST_ID_HEADER: &str = "X-Request-Id";

/// Configuration for correlation ID rate limiting
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitConfig {
    /// Maximum length allowed for correlation IDs
    pub max_correlation_id_length: usize,
    /// Maximum requests per correlation ID per time window
    pub max_requests_per_correlation_id: usize,
    /// Time window for rate limiting in seconds
    pub rate_limit_window_secs: u64,
    /// Enable rate limiting (can be disabled for testing/debugging)
    pub enabled: bool,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            max_correlation_id_length: 200,
            max_requests_per_correlation_id: 100,
            rate_limit_window_secs: 60,
            enabled: true,
        }
    }
}

impl RateLimitConfig {
    /// Get the rate limit window as a Duration
    pub fn rate_limit_window(&self) -> Duration {
        Duration::from_secs(self.rate_limit_window_secs)
    }

    /// Create a permissive config for testing environments
    pub fn permissive() -> Self {
        Self {
            max_correlation_id_length: 500,
            max_requests_per_correlation_id: 10000,
            rate_limit_window_secs: 1,
            enabled: false,
        }
    }

    /// Create a strict config for production environments
    pub fn strict() -> Self {
        Self {
            max_correlation_id_length: 100,
            max_requests_per_correlation_id: 50,
            rate_limit_window_secs: 60,
            enabled: true,
        }
    }
}

#[derive(Debug, Clone)]
struct RateLimitEntry {
    count: usize,
    window_start: Instant,
}

// Simple in-memory rate limiter for correlation IDs
static CORRELATION_RATE_LIMITER: std::sync::OnceLock<Arc<Mutex<HashMap<String, RateLimitEntry>>>> =
    std::sync::OnceLock::new();

#[derive(Debug, Clone)]
pub struct RequestContext {
    pub correlation_id: String,
    pub request_id: String,
}

impl RequestContext {
    pub fn new(correlation_id: Option<String>) -> Self {
        Self {
            correlation_id: correlation_id.unwrap_or_else(|| Uuid::new_v4().to_string()),
            request_id: Uuid::new_v4().to_string(),
        }
    }
}

/// Validate correlation ID format and length
fn validate_correlation_id(
    correlation_id: &str,
    config: &RateLimitConfig,
) -> Result<(), &'static str> {
    if correlation_id.is_empty() {
        return Err("Correlation ID cannot be empty");
    }

    if correlation_id.len() > config.max_correlation_id_length {
        return Err("Correlation ID exceeds maximum length");
    }

    // Check for valid characters (alphanumeric, hyphens, underscores)
    if !correlation_id
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        return Err("Correlation ID contains invalid characters");
    }

    Ok(())
}

/// Check rate limit for correlation ID
fn check_rate_limit(correlation_id: &str, config: &RateLimitConfig) -> Result<(), &'static str> {
    // Skip rate limiting if disabled
    if !config.enabled {
        return Ok(());
    }

    let rate_limiter =
        CORRELATION_RATE_LIMITER.get_or_init(|| Arc::new(Mutex::new(HashMap::new())));

    let mut limiter = match rate_limiter.lock() {
        Ok(limiter) => limiter,
        Err(_) => {
            // If mutex is poisoned, we still reject to be safe
            return Err("Rate limiter unavailable");
        }
    };
    let now = Instant::now();
    let rate_limit_window = config.rate_limit_window();

    // Clean up expired entries
    limiter.retain(|_, entry| now.duration_since(entry.window_start) < rate_limit_window);

    match limiter.get_mut(correlation_id) {
        Some(entry) => {
            if now.duration_since(entry.window_start) >= rate_limit_window {
                // Reset window
                entry.count = 1;
                entry.window_start = now;
                Ok(())
            } else if entry.count >= config.max_requests_per_correlation_id {
                Err("Rate limit exceeded for correlation ID")
            } else {
                entry.count += 1;
                Ok(())
            }
        }
        None => {
            limiter.insert(
                correlation_id.to_string(),
                RateLimitEntry {
                    count: 1,
                    window_start: now,
                },
            );
            Ok(())
        }
    }
}

pub fn create_request_id_middleware(
    config: RateLimitConfig,
) -> impl Fn(
    Request,
    Next,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<Response, StatusCode>> + Send>,
> + Clone {
    move |req, next| {
        let config = config.clone();
        Box::pin(request_id_middleware_impl(req, next, config))
    }
}

/// Default middleware with default configuration
pub async fn request_id_middleware(req: Request, next: Next) -> Result<Response, StatusCode> {
    request_id_middleware_impl(req, next, RateLimitConfig::default()).await
}

async fn request_id_middleware_impl(
    mut req: Request,
    next: Next,
    config: RateLimitConfig,
) -> Result<Response, StatusCode> {
    // Extract correlation ID from headers with validation
    let correlation_id = req
        .headers()
        .get(CORRELATION_ID_HEADER)
        .and_then(|h| h.to_str().ok())
        .map(|s| s.to_string());

    let validated_correlation_id = match correlation_id {
        Some(id) => {
            // Validate correlation ID format
            if let Err(reason) = validate_correlation_id(&id, &config) {
                warn!(
                    correlation_id = %id,
                    reason = %reason,
                    "Invalid correlation ID rejected"
                );
                return Err(StatusCode::BAD_REQUEST);
            }

            // Check rate limit
            if let Err(reason) = check_rate_limit(&id, &config) {
                warn!(
                    correlation_id = %id,
                    reason = %reason,
                    "Correlation ID rate limit exceeded"
                );
                return Err(StatusCode::TOO_MANY_REQUESTS);
            }

            Some(id)
        }
        None => None,
    };

    let context = RequestContext::new(validated_correlation_id);

    // Add to request extensions for handlers to access
    req.extensions_mut().insert(context.clone());

    // Create span with correlation and request IDs
    let span = info_span!(
        "request",
        correlation_id = %context.correlation_id,
        request_id = %context.request_id,
        method = %req.method(),
        uri = %req.uri().path(),
        version = ?req.version(),
    );

    // Process request within the span
    async move {
        let mut response = next.run(req).await;

        // Add IDs to response headers for debugging
        response.headers_mut().insert(
            CORRELATION_ID_HEADER,
            HeaderValue::from_str(&context.correlation_id)
                .unwrap_or_else(|_| HeaderValue::from_static("invalid-correlation-id")),
        );
        response.headers_mut().insert(
            REQUEST_ID_HEADER,
            HeaderValue::from_str(&context.request_id)
                .unwrap_or_else(|_| HeaderValue::from_static("invalid-request-id")),
        );

        Ok(response)
    }
    .instrument(span)
    .await
}
