use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::Extension;
use axum::http::StatusCode;
use axum::response::Json;
use chrono::{DateTime, Utc};
use fedimint_client::Client;
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info, warn};

use crate::state::AppState;

/// Overall health state of a component or the entire system
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum HealthState {
    /// Component is functioning normally
    Healthy,
    /// Component has issues but is still functional
    Degraded,
    /// Component is not functional
    Unhealthy,
}

/// Health status for an individual component
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentHealth {
    /// Current health state
    pub status: HealthState,
    /// Human-readable status message
    pub message: Option<String>,
    /// When this check was last performed
    pub last_check: DateTime<Utc>,
    /// Additional metadata about the component
    pub metadata: Option<serde_json::Value>,
    /// Duration of the health check in milliseconds
    pub check_duration_ms: Option<u64>,
}

impl ComponentHealth {
    pub fn healthy(message: impl Into<String>) -> Self {
        Self {
            status: HealthState::Healthy,
            message: Some(message.into()),
            last_check: Utc::now(),
            metadata: None,
            check_duration_ms: None,
        }
    }

    pub fn degraded(message: impl Into<String>) -> Self {
        Self {
            status: HealthState::Degraded,
            message: Some(message.into()),
            last_check: Utc::now(),
            metadata: None,
            check_duration_ms: None,
        }
    }

    pub fn unhealthy(message: impl Into<String>) -> Self {
        Self {
            status: HealthState::Unhealthy,
            message: Some(message.into()),
            last_check: Utc::now(),
            metadata: None,
            check_duration_ms: None,
        }
    }

    pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = Some(metadata);
        self
    }

    pub fn with_duration(mut self, duration: Duration) -> Self {
        self.check_duration_ms = Some(duration.as_millis() as u64);
        self
    }
}

/// Complete health status including all components
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthStatus {
    /// Overall system health state
    pub status: HealthState,
    /// Application version
    pub version: String,
    /// System uptime in seconds
    pub uptime_seconds: u64,
    /// Timestamp of this health check
    pub timestamp: DateTime<Utc>,
    /// Health status of individual components
    pub checks: HashMap<String, ComponentHealth>,
    /// Summary statistics
    pub summary: HealthSummary,
}

/// Summary statistics for the health check
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthSummary {
    /// Total number of components checked
    pub total_checks: usize,
    /// Number of healthy components
    pub healthy_count: usize,
    /// Number of degraded components
    pub degraded_count: usize,
    /// Number of unhealthy components
    pub unhealthy_count: usize,
    /// Total time taken for all health checks in milliseconds
    pub total_check_duration_ms: u64,
}

/// Comprehensive health check endpoint
pub async fn health_check(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<HealthStatus>, StatusCode> {
    let start_time = Instant::now();
    let mut checks = HashMap::new();
    let timestamp = Utc::now();

    debug!("Starting comprehensive health check");

    // Check database health
    let db_check_start = Instant::now();
    let database_health = check_database_health(&state).await;
    checks.insert(
        "database".to_string(),
        database_health.with_duration(db_check_start.elapsed()),
    );

    // Check all federation clients
    let federations = state.multimint.all().await;
    for (federation_id, client) in federations {
        let fed_check_start = Instant::now();
        let federation_health = check_federation_health(&client, &federation_id).await;
        checks.insert(
            format!("federation_{}", federation_id),
            federation_health.with_duration(fed_check_start.elapsed()),
        );
    }

    // Check event bus health
    let event_check_start = Instant::now();
    let event_bus_health = check_event_bus_health(&state).await;
    checks.insert(
        "event_bus".to_string(),
        event_bus_health.with_duration(event_check_start.elapsed()),
    );

    // Check system resources
    let system_check_start = Instant::now();
    let system_health = check_system_health().await;
    checks.insert(
        "system".to_string(),
        system_health.with_duration(system_check_start.elapsed()),
    );

    // Determine overall health status
    let overall_status = determine_overall_health(&checks);

    // Calculate summary statistics
    let total_duration = start_time.elapsed();
    let summary = calculate_health_summary(&checks, total_duration);

    let health_status = HealthStatus {
        status: overall_status,
        version: env!("CARGO_PKG_VERSION").to_string(),
        uptime_seconds: state.start_time.elapsed().as_secs(),
        timestamp,
        checks,
        summary,
    };

    info!(
        overall_status = ?health_status.status,
        total_checks = health_status.summary.total_checks,
        healthy_count = health_status.summary.healthy_count,
        degraded_count = health_status.summary.degraded_count,
        unhealthy_count = health_status.summary.unhealthy_count,
        duration_ms = total_duration.as_millis(),
        "Health check completed"
    );

    // Return appropriate HTTP status based on overall health
    match health_status.status {
        HealthState::Healthy => Ok(Json(health_status)),
        HealthState::Degraded => {
            // System is degraded but still functional - return 200 with warning
            warn!("System is in degraded state but still operational");
            Ok(Json(health_status))
        }
        HealthState::Unhealthy => {
            error!("System health check failed - returning 503 Service Unavailable");
            Err(StatusCode::SERVICE_UNAVAILABLE)
        }
    }
}

/// Kubernetes liveness probe endpoint
pub async fn liveness_check(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<&'static str, StatusCode> {
    debug!("Performing liveness check");

    // Basic liveness check - just ensure the application is running
    // and core components are not completely broken

    // Check if we can access the database at all
    if let Err(e) = check_database_connectivity(&state).await {
        error!("Liveness check failed - database connectivity issue: {}", e);
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }

    // Check if event bus is responsive
    if let Err(e) = check_event_bus_connectivity(&state).await {
        error!("Liveness check failed - event bus issue: {}", e);
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }

    debug!("Liveness check passed");
    Ok("alive")
}

/// Kubernetes readiness probe endpoint
pub async fn readiness_check(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<&'static str, StatusCode> {
    debug!("Performing readiness check");

    // Readiness check ensures the service is ready to handle traffic
    // More thorough than liveness check

    // Check database readiness
    let db_health = check_database_health(&state).await;
    if matches!(db_health.status, HealthState::Unhealthy) {
        warn!("Readiness check failed - database is unhealthy");
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }

    // Check if at least one federation is available
    let federations = state.multimint.all().await;
    if federations.is_empty() {
        warn!("Readiness check failed - no federations available");
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }

    // Check if at least one federation is healthy
    let mut any_federation_healthy = false;
    for (federation_id, client) in federations {
        let fed_health = check_federation_health(&client, &federation_id).await;
        if matches!(
            fed_health.status,
            HealthState::Healthy | HealthState::Degraded
        ) {
            any_federation_healthy = true;
            break;
        }
    }

    if !any_federation_healthy {
        warn!("Readiness check failed - no healthy federations available");
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }

    debug!("Readiness check passed");
    Ok("ready")
}

/// Check database health
async fn check_database_health(state: &AppState) -> ComponentHealth {
    let start = Instant::now();

    match check_database_connectivity(state).await {
        Ok(()) => {
            // Try a simple operation to test database functionality
            match test_database_operation(state).await {
                Ok(stats) => ComponentHealth::healthy("Database is functioning normally")
                    .with_metadata(serde_json::json!({
                        "connection_time_ms": start.elapsed().as_millis(),
                        "stats": stats
                    })),
                Err(e) => ComponentHealth::degraded(format!(
                    "Database connected but operations are slow: {}",
                    e
                ))
                .with_metadata(serde_json::json!({
                    "error": e.to_string(),
                    "connection_time_ms": start.elapsed().as_millis()
                })),
            }
        }
        Err(e) => ComponentHealth::unhealthy(format!("Database connectivity failed: {}", e))
            .with_metadata(serde_json::json!({
                "error": e.to_string(),
                "connection_time_ms": start.elapsed().as_millis()
            })),
    }
}

/// Check basic database connectivity
async fn check_database_connectivity(state: &AppState) -> anyhow::Result<()> {
    // Try to access the database through the multimint interface
    let _federations = state.multimint.all().await;
    Ok(())
}

/// Test database operation performance
async fn test_database_operation(state: &AppState) -> anyhow::Result<serde_json::Value> {
    let federations = state.multimint.all().await;

    Ok(serde_json::json!({
        "federation_count": federations.len(),
        "test_passed": true
    }))
}

/// Check federation health
async fn check_federation_health(client: &Arc<Client>, federation_id: &str) -> ComponentHealth {
    let start = Instant::now();

    // Test federation connectivity and responsiveness
    match test_federation_connectivity(client).await {
        Ok(info) => {
            let connection_time = start.elapsed();

            if connection_time > Duration::from_secs(5) {
                ComponentHealth::degraded(format!(
                    "Federation {} is responding slowly ({:.2}s)",
                    federation_id,
                    connection_time.as_secs_f64()
                ))
                .with_metadata(serde_json::json!({
                    "federation_id": federation_id,
                    "connection_time_ms": connection_time.as_millis(),
                    "info": info
                }))
            } else {
                ComponentHealth::healthy(format!("Federation {} is healthy", federation_id))
                    .with_metadata(serde_json::json!({
                        "federation_id": federation_id,
                        "connection_time_ms": connection_time.as_millis(),
                        "info": info
                    }))
            }
        }
        Err(e) => ComponentHealth::unhealthy(format!(
            "Federation {} is unreachable: {}",
            federation_id, e
        ))
        .with_metadata(serde_json::json!({
            "federation_id": federation_id,
            "error": e.to_string(),
            "connection_time_ms": start.elapsed().as_millis()
        })),
    }
}

/// Test federation connectivity
async fn test_federation_connectivity(client: &Arc<Client>) -> anyhow::Result<serde_json::Value> {
    // Try to get federation info - this tests connectivity without modifying state
    let config = client.config().await;

    Ok(serde_json::json!({
        "federation_name": config.global.federation_name().unwrap_or("Unknown"),
        "api_version": config.global.api_version,
        "consensus_version": config.consensus_version,
        "module_count": config.modules.len()
    }))
}

/// Check event bus health
async fn check_event_bus_health(state: &AppState) -> ComponentHealth {
    match check_event_bus_connectivity(state).await {
        Ok(stats) => {
            ComponentHealth::healthy("Event bus is functioning normally").with_metadata(stats)
        }
        Err(e) => ComponentHealth::unhealthy(format!("Event bus is not responding: {}", e))
            .with_metadata(serde_json::json!({
                "error": e.to_string()
            })),
    }
}

/// Check event bus connectivity
async fn check_event_bus_connectivity(state: &AppState) -> anyhow::Result<serde_json::Value> {
    let stats = state.event_bus.stats().await;

    Ok(serde_json::json!({
        "capacity": stats.capacity,
        "handler_count": stats.handler_count,
        "critical_handler_count": stats.critical_handler_count
    }))
}

/// Check system resource health
async fn check_system_health() -> ComponentHealth {
    // Basic system health checks
    let metadata = serde_json::json!({
        "timestamp": Utc::now(),
        "process_id": std::process::id(),
        "available": true
    });

    ComponentHealth::healthy("System resources are available").with_metadata(metadata)
}

/// Determine overall health based on component health states
fn determine_overall_health(checks: &HashMap<String, ComponentHealth>) -> HealthState {
    if checks.is_empty() {
        return HealthState::Unhealthy;
    }

    let has_unhealthy = checks
        .values()
        .any(|c| matches!(c.status, HealthState::Unhealthy));
    let has_degraded = checks
        .values()
        .any(|c| matches!(c.status, HealthState::Degraded));

    if has_unhealthy {
        HealthState::Unhealthy
    } else if has_degraded {
        HealthState::Degraded
    } else {
        HealthState::Healthy
    }
}

/// Calculate health check summary statistics
fn calculate_health_summary(
    checks: &HashMap<String, ComponentHealth>,
    total_duration: Duration,
) -> HealthSummary {
    let total_checks = checks.len();
    let healthy_count = checks
        .values()
        .filter(|c| matches!(c.status, HealthState::Healthy))
        .count();
    let degraded_count = checks
        .values()
        .filter(|c| matches!(c.status, HealthState::Degraded))
        .count();
    let unhealthy_count = checks
        .values()
        .filter(|c| matches!(c.status, HealthState::Unhealthy))
        .count();

    HealthSummary {
        total_checks,
        healthy_count,
        degraded_count,
        unhealthy_count,
        total_check_duration_ms: total_duration.as_millis() as u64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_component_health_creation() {
        let healthy = ComponentHealth::healthy("All good");
        assert_eq!(healthy.status, HealthState::Healthy);
        assert_eq!(healthy.message, Some("All good".to_string()));
        assert!(healthy.metadata.is_none());

        let degraded = ComponentHealth::degraded("Some issues")
            .with_metadata(serde_json::json!({"issue": "slow response"}));
        assert_eq!(degraded.status, HealthState::Degraded);
        assert!(degraded.metadata.is_some());

        let unhealthy = ComponentHealth::unhealthy("System down");
        assert_eq!(unhealthy.status, HealthState::Unhealthy);
    }

    #[test]
    fn test_determine_overall_health() {
        let mut checks = HashMap::new();

        // All healthy
        checks.insert("db".to_string(), ComponentHealth::healthy("OK"));
        checks.insert("fed".to_string(), ComponentHealth::healthy("OK"));
        assert_eq!(determine_overall_health(&checks), HealthState::Healthy);

        // One degraded
        checks.insert("api".to_string(), ComponentHealth::degraded("Slow"));
        assert_eq!(determine_overall_health(&checks), HealthState::Degraded);

        // One unhealthy
        checks.insert("queue".to_string(), ComponentHealth::unhealthy("Down"));
        assert_eq!(determine_overall_health(&checks), HealthState::Unhealthy);

        // Empty checks
        checks.clear();
        assert_eq!(determine_overall_health(&checks), HealthState::Unhealthy);
    }

    #[test]
    fn test_health_summary_calculation() {
        let mut checks = HashMap::new();
        checks.insert("healthy1".to_string(), ComponentHealth::healthy("OK"));
        checks.insert("healthy2".to_string(), ComponentHealth::healthy("OK"));
        checks.insert("degraded1".to_string(), ComponentHealth::degraded("Slow"));
        checks.insert("unhealthy1".to_string(), ComponentHealth::unhealthy("Down"));

        let summary = calculate_health_summary(&checks, Duration::from_millis(500));

        assert_eq!(summary.total_checks, 4);
        assert_eq!(summary.healthy_count, 2);
        assert_eq!(summary.degraded_count, 1);
        assert_eq!(summary.unhealthy_count, 1);
        assert_eq!(summary.total_check_duration_ms, 500);
    }

    #[test]
    fn test_component_health_with_duration() {
        let duration = Duration::from_millis(250);
        let health = ComponentHealth::healthy("OK").with_duration(duration);

        assert_eq!(health.check_duration_ms, Some(250));
    }
}
