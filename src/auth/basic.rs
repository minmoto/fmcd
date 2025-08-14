use std::sync::Arc;

use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::Response;
use base64::Engine;
use tracing::{info, warn};

#[derive(Clone)]
pub struct BasicAuth {
    username: String,
    password: String,
    enabled: bool,
}

impl BasicAuth {
    /// Create new BasicAuth instance following phoenixd model
    /// Uses fixed username "fmcd" and optional password
    pub fn new(password: Option<String>) -> Self {
        Self {
            username: "fmcd".to_string(), // Fixed username like phoenixd
            password: password.clone().unwrap_or_default(),
            enabled: password.is_some(),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn verify(&self, auth_header: &str) -> bool {
        if !self.enabled {
            return true;
        }

        if !auth_header.starts_with("Basic ") {
            return false;
        }

        let credentials = &auth_header[6..];
        match base64::engine::general_purpose::STANDARD.decode(credentials) {
            Ok(decoded) => {
                let decoded_str = String::from_utf8_lossy(&decoded);
                decoded_str == format!("{}:{}", self.username, self.password)
            }
            Err(_) => false,
        }
    }
}

pub async fn basic_auth_middleware(
    auth: Arc<BasicAuth>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let method = request.method().to_string();
    let path = request.uri().path().to_string();
    let remote_addr = request
        .headers()
        .get("x-forwarded-for")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("unknown");

    // If authentication is disabled, pass through
    if !auth.enabled {
        info!(
            method = %method,
            path = %path,
            remote_addr = %remote_addr,
            auth_enabled = false,
            auth_result = "bypassed",
            "Authentication bypassed - auth disabled"
        );
        return Ok(next.run(request).await);
    }

    let auth_header = request
        .headers()
        .get("Authorization")
        .and_then(|header| header.to_str().ok());

    match auth_header {
        Some(header) if auth.verify(header) => {
            info!(
                method = %method,
                path = %path,
                remote_addr = %remote_addr,
                auth_enabled = true,
                auth_result = "success",
                auth_type = "basic",
                username = %auth.username,
                "Authentication successful"
            );
            Ok(next.run(request).await)
        }
        Some(_) => {
            warn!(
                method = %method,
                path = %path,
                remote_addr = %remote_addr,
                auth_enabled = true,
                auth_result = "failure",
                auth_type = "basic",
                failure_reason = "invalid_credentials",
                "Authentication failed - invalid credentials"
            );
            let response = Response::builder()
                .status(StatusCode::UNAUTHORIZED)
                .header("WWW-Authenticate", "Basic realm=\"fmcd\"")
                .body(Body::from("Unauthorized"))
                .unwrap_or_else(|_| Response::new(Body::from("Unauthorized")));
            Ok(response)
        }
        None => {
            warn!(
                method = %method,
                path = %path,
                remote_addr = %remote_addr,
                auth_enabled = true,
                auth_result = "failure",
                auth_type = "basic",
                failure_reason = "missing_authorization_header",
                "Authentication failed - missing Authorization header"
            );
            let response = Response::builder()
                .status(StatusCode::UNAUTHORIZED)
                .header("WWW-Authenticate", "Basic realm=\"fmcd\"")
                .body(Body::from("Unauthorized"))
                .unwrap_or_else(|_| Response::new(Body::from("Unauthorized")));
            Ok(response)
        }
    }
}
