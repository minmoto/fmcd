use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use tracing::{info, warn};

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone)]
pub struct WebSocketAuth {
    secret: Vec<u8>,
    enabled: bool,
}

impl WebSocketAuth {
    /// Create new WebSocketAuth instance
    /// Uses the same password as HTTP Basic Auth for simplicity
    pub fn new(password: Option<String>) -> Self {
        Self {
            secret: password.clone().unwrap_or_default().as_bytes().to_vec(),
            enabled: password.is_some(),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Create HMAC-SHA256 signature for a message with timestamp
    pub fn create_signature(&self, message: &str, timestamp: i64) -> Result<String, String> {
        if !self.enabled {
            return Ok(String::new());
        }

        let mut mac = HmacSha256::new_from_slice(&self.secret)
            .map_err(|e| format!("Failed to create HMAC: {}", e))?;

        let payload = format!("{}{}", timestamp, message);
        mac.update(payload.as_bytes());

        Ok(hex::encode(mac.finalize().into_bytes()))
    }

    /// Verify HMAC signature for a message with timestamp
    /// No replay protection - just signature verification
    pub fn verify_signature(&self, message: &str, timestamp: i64, signature: &str) -> bool {
        if !self.enabled {
            info!(
                auth_enabled = false,
                auth_result = "bypassed",
                auth_type = "hmac",
                "WebSocket authentication bypassed - auth disabled"
            );
            return true;
        }

        match self.create_signature(message, timestamp) {
            Ok(expected) => {
                if expected == signature {
                    info!(
                        auth_enabled = true,
                        auth_result = "success",
                        auth_type = "hmac",
                        timestamp = timestamp,
                        "WebSocket authentication successful"
                    );
                    true
                } else {
                    warn!(
                        auth_enabled = true,
                        auth_result = "failure",
                        auth_type = "hmac",
                        failure_reason = "invalid_signature",
                        timestamp = timestamp,
                        "WebSocket authentication failed - invalid signature"
                    );
                    false
                }
            }
            Err(err) => {
                warn!(
                    auth_enabled = true,
                    auth_result = "failure",
                    auth_type = "hmac",
                    failure_reason = "signature_creation_error",
                    timestamp = timestamp,
                    error = %err,
                    "WebSocket authentication failed - signature creation error"
                );
                false
            }
        }
    }
}

/// WebSocket message format with HMAC authentication
#[derive(Serialize, Deserialize, Debug)]
pub struct AuthenticatedMessage {
    pub timestamp: i64,
    pub signature: String,
    pub payload: serde_json::Value,
}

impl AuthenticatedMessage {
    /// Create new authenticated message
    pub fn new(payload: serde_json::Value, auth: &WebSocketAuth) -> Result<Self, String> {
        let timestamp = chrono::Utc::now().timestamp();
        let payload_str = serde_json::to_string(&payload)
            .map_err(|e| format!("Failed to serialize payload: {}", e))?;

        let signature = auth.create_signature(&payload_str, timestamp)?;

        Ok(Self {
            timestamp,
            signature,
            payload,
        })
    }

    /// Verify the message signature
    pub fn verify(&self, auth: &WebSocketAuth) -> bool {
        let payload_str = match serde_json::to_string(&self.payload) {
            Ok(s) => s,
            Err(_) => return false,
        };

        auth.verify_signature(&payload_str, self.timestamp, &self.signature)
    }
}
