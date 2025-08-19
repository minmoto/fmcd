use std::path::{Path, PathBuf};

use anyhow::Result;
use rand::RngCore;
use serde::{Deserialize, Serialize};

use crate::observability::correlation::RateLimitConfig;
use crate::webhooks::WebhookConfig;

/// Configuration structure
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    /// HTTP server bind IP address
    #[serde(rename = "http-bind-ip", default = "default_bind_ip")]
    pub http_bind_ip: String,

    /// HTTP server bind port
    #[serde(rename = "http-bind-port", default = "default_bind_port")]
    pub http_bind_port: u16,

    /// HTTP Basic Auth password (plain text, optional)
    /// When None, authentication is disabled
    #[serde(rename = "http-password")]
    pub http_password: Option<String>,

    /// WebSocket enabled flag
    #[serde(rename = "websocket-enabled", default = "default_websocket_enabled")]
    pub websocket_enabled: bool,

    /// WebSocket port (if different from HTTP)
    #[serde(rename = "websocket-port")]
    pub websocket_port: Option<u16>,

    /// Data directory for the daemon (contains database and config)
    #[serde(rename = "data-dir")]
    pub data_dir: Option<PathBuf>,

    /// Federation invite code
    #[serde(rename = "invite-code")]
    pub invite_code: Option<String>,

    /// Manual secret for additional security
    #[serde(rename = "manual-secret")]
    pub manual_secret: Option<String>,

    /// Webhook configuration
    #[serde(rename = "webhooks", default)]
    pub webhooks: WebhookConfig,

    /// Rate limiting configuration for correlation IDs
    #[serde(rename = "rate-limiting", default)]
    pub rate_limiting: RateLimitConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            http_bind_ip: default_bind_ip(),
            http_bind_port: default_bind_port(),
            http_password: None,
            websocket_enabled: default_websocket_enabled(),
            websocket_port: None,
            data_dir: None,
            invite_code: None,
            manual_secret: None,
            webhooks: WebhookConfig::default(),
            rate_limiting: RateLimitConfig::default(),
        }
    }
}

impl Config {
    /// Load configuration from TOML file
    pub fn load_from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();

        // Ensure parent directory exists (important for Docker volumes)
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let contents = std::fs::read_to_string(path)?;
        let config: Config = toml::from_str(&contents)?;
        Ok(config)
    }

    /// Save configuration to TOML file atomically
    pub fn save_to_file<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let path = path.as_ref();

        // Ensure parent directory exists (important for Docker volumes)
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let contents = toml::to_string_pretty(self)?;

        // Write to temporary file first
        let temp_path = path.with_extension("tmp");
        std::fs::write(&temp_path, contents)?;

        // Atomically rename temp file to actual config file
        // This ensures the config file is never in a partially written state
        match std::fs::rename(&temp_path, path) {
            Ok(_) => Ok(()),
            Err(e) => {
                // Clean up temp file if rename failed
                let _ = std::fs::remove_file(&temp_path);
                Err(e.into())
            }
        }
    }

    /// Get the complete HTTP server address
    pub fn http_address(&self) -> String {
        format!("{}:{}", self.http_bind_ip, self.http_bind_port)
    }

    /// Get the WebSocket address (same as HTTP if not specified)
    pub fn websocket_address(&self) -> String {
        let port = self.websocket_port.unwrap_or(self.http_bind_port);
        format!("{}:{}", self.http_bind_ip, port)
    }

    /// Check if authentication is enabled
    pub fn is_auth_enabled(&self) -> bool {
        self.http_password.is_some()
    }

    /// Get the authentication password
    pub fn auth_password(&self) -> Option<&str> {
        self.http_password.as_deref()
    }

    /// Generate a secure random 32-byte hex password
    pub fn generate_password() -> String {
        let mut bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut bytes);
        hex::encode(bytes)
    }

    /// Load or create configuration file with automatic password generation
    /// Uses atomic file operations to prevent password loss on crash
    pub fn load_or_create<P: AsRef<Path>>(path: P) -> Result<(Self, bool)> {
        let path = path.as_ref();
        let mut password_generated = false;

        // Ensure parent directory exists (important for Docker volumes)
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let mut config = if path.exists() {
            match Self::load_from_file(path) {
                Ok(cfg) => cfg,
                Err(_) => {
                    // If config file is corrupted, recreate it
                    let cfg = Self::default();
                    cfg.save_to_file(path)?;
                    cfg
                }
            }
        } else {
            let config = Self::default();
            config.save_to_file(path)?;
            config
        };

        // Check if we need to generate password
        if config.http_password.is_none() {
            let generated_password = Self::generate_password();
            config.http_password = Some(generated_password);
            password_generated = true;

            // Save the complete config with the password properly in the structure
            config.save_to_file(path)?;
        }

        Ok((config, password_generated))
    }
}

// Default value functions
fn default_bind_ip() -> String {
    // Use 0.0.0.0 in containerized environments to allow external connections
    // Check for common container environment indicators
    if std::env::var("DOCKER_CONTAINER").is_ok()
        || std::env::var("FMCD_ADDR").is_ok()
        || std::path::Path::new("/.dockerenv").exists()
        || std::env::var("KUBERNETES_SERVICE_HOST").is_ok()
    {
        "0.0.0.0".to_string()
    } else {
        "127.0.0.1".to_string()
    }
}

fn default_bind_port() -> u16 {
    7070
}

fn default_websocket_enabled() -> bool {
    true
}
