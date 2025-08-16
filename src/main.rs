use std::future::ready;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Instant;

use anyhow::Result;
use axum::extract::{MatchedPath, Request};
use axum::http::Method;
use axum::middleware::{self, Next};
use axum::response::IntoResponse;
use fedimint_core::invite_code::InviteCode;
use futures::future::TryFutureExt;
use metrics_exporter_prometheus::{Matcher, PrometheusBuilder, PrometheusHandle};
use router::handlers::{admin, ln, mint, onchain};
use router::ws::websocket_handler;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing::info;

use crate::health::{health_check, liveness_check, readiness_check};
use crate::metrics::{api_metrics, init_prometheus_metrics};

mod auth;
mod config;
mod multimint;
mod observability;

mod error;
mod health;
mod metrics;
mod router;
mod state;
mod utils;

use std::sync::Arc;

use auth::{basic_auth_middleware, BasicAuth, WebSocketAuth};
use axum::routing::{get, post};
use axum::Router;
use clap::{Parser, Subcommand, ValueEnum};
use config::Config;
use console::{style, Term};
use observability::{init_logging, request_id_middleware, LoggingConfig};
use state::AppState;

#[derive(Clone, Debug, ValueEnum, PartialEq)]
enum Mode {
    Rest,
    Ws,
}

impl FromStr for Mode {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "rest" => Ok(Mode::Rest),
            "ws" => Ok(Mode::Ws),
            _ => Err(anyhow::anyhow!("Invalid mode")),
        }
    }
}

#[derive(Subcommand)]
enum Commands {
    Start,
    Stop,
}

#[derive(Parser)]
#[clap(version = "1.0", author = "Kody Low")]
struct Cli {
    /// Data directory path (contains config and database)
    #[clap(long, env = "FMCD_DATA_DIR", default_value = ".")]
    data_dir: PathBuf,

    /// Federation invite code (overrides config)
    #[clap(long, env = "FMCD_INVITE_CODE")]
    invite_code: Option<String>,

    /// Password (overrides config)
    #[clap(long, env = "FMCD_PASSWORD")]
    password: Option<String>,

    /// Server address (overrides config)
    #[clap(long, env = "FMCD_ADDR")]
    addr: Option<String>,

    /// Manual secret (overrides config)
    #[clap(long, env = "FMCD_MANUAL_SECRET")]
    manual_secret: Option<String>,

    /// Mode: ws, rest
    #[clap(long, env = "FMCD_MODE", default_value = "rest")]
    mode: Mode,

    /// Disable authentication
    #[clap(long)]
    no_auth: bool,
}

// const PID_FILE: &str = "/tmp/fedimint_http.pid";

#[tokio::main]
async fn main() -> Result<()> {
    dotenv::dotenv().ok();

    let cli: Cli = Cli::parse();

    // Initialize structured logging
    let log_config = LoggingConfig {
        level: std::env::var("LOG_LEVEL").unwrap_or_else(|_| "info".to_string()),
        log_dir: cli.data_dir.join("logs"),
        console_output: !std::env::var("NO_CONSOLE_LOG").is_ok(),
        file_output: !std::env::var("NO_FILE_LOG").is_ok(),
        ..Default::default()
    };
    init_logging(log_config)?;

    tracing::info!("Starting FMCD with structured logging and observability");

    // Ensure data directory exists
    std::fs::create_dir_all(&cli.data_dir)?;

    // Config file is always in data_dir
    let config_path = cli.data_dir.join("fmcd.conf");

    // Load or create configuration file with automatic password generation
    let term = Term::stdout();
    let (mut config, password_generated) = Config::load_or_create(&config_path)?;

    if password_generated {
        term.write_line(&format!(
            "{}{}",
            style("Generating default api password...").yellow(),
            style("done").white()
        ))?;
    }

    // Override config with CLI arguments
    if let Some(invite_code) = cli.invite_code {
        config.invite_code = Some(invite_code);
    }
    // Update config's data_dir to match CLI
    config.data_dir = Some(cli.data_dir.clone());
    if let Some(password) = cli.password {
        config.http_password = Some(password);
    }
    if let Some(addr) = cli.addr {
        // Parse address to extract IP and port
        if let Some((ip, port_str)) = addr.split_once(':') {
            config.http_bind_ip = ip.to_string();
            if let Ok(port) = port_str.parse::<u16>() {
                config.http_bind_port = port;
            }
        }
    }
    if let Some(manual_secret) = cli.manual_secret {
        config.manual_secret = Some(manual_secret);
    }
    if cli.no_auth {
        config.http_password = None;
    }

    // Database path is always the data directory
    let db_path = cli.data_dir.clone();

    let mut state = AppState::new_with_config(db_path, config.webhooks.clone()).await?;

    // Handle federation invite code
    if let Some(invite_code_str) = &config.invite_code {
        match InviteCode::from_str(invite_code_str) {
            Ok(invite_code) => {
                let federation_id = state.multimint.register_new(invite_code).await?;
                info!("Created client for federation id: {:?}", federation_id);
            }
            Err(e) => {
                info!(
                    "No federation invite code provided, skipping client creation: {}",
                    e
                );
            }
        }
    }

    if state.multimint.all().await.is_empty() {
        return Err(anyhow::anyhow!("No clients found, must have at least one client to start the server. Try providing a federation invite code with the `--invite-code` flag or setting the `FMCD_INVITE_CODE` environment variable."));
    }

    // Start monitoring services for full observability parity
    if let Err(e) = state.start_monitoring_services().await {
        tracing::warn!("Failed to start monitoring services: {}", e);
    } else {
        tracing::info!("Monitoring services started successfully");
    }

    start_main_server(&config, cli.mode, state).await?;
    Ok(())
}

async fn start_main_server(config: &Config, mode: Mode, state: AppState) -> anyhow::Result<()> {
    // Create authentication instances
    let basic_auth = Arc::new(BasicAuth::new(config.http_password.clone()));
    let ws_auth = Arc::new(WebSocketAuth::new(config.http_password.clone()));

    // Create the router based on mode
    let app = match mode {
        Mode::Rest => {
            let router = Router::new()
                .nest("/v2", fedimint_v2_rest())
                .with_state(state);

            // Apply authentication middleware if enabled
            if basic_auth.is_enabled() {
                let auth_clone = basic_auth.clone();
                router.route_layer(middleware::from_fn(move |request, next| {
                    basic_auth_middleware(auth_clone.clone(), request, next)
                }))
            } else {
                router
            }
        }
        Mode::Ws => Router::new()
            .route("/ws", get(websocket_handler))
            .with_state(state.clone())
            .layer(axum::Extension(ws_auth)),
    };

    let auth_status = if config.is_auth_enabled() {
        "enabled"
    } else {
        "disabled"
    };
    info!("Starting server in {mode:?} mode with authentication {auth_status}");

    let cors = CorsLayer::new()
        .allow_methods([Method::GET, Method::POST])
        .allow_origin(Any)
        .allow_headers(Any);

    // Initialize comprehensive metrics system
    let metrics_handle = init_prometheus_metrics().await?;

    let app = app
        .layer(middleware::from_fn(request_id_middleware))
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .route("/health", get(health_check))
        .route("/health/live", get(liveness_check))
        .route("/health/ready", get(readiness_check))
        .route("/metrics", get(move || ready(metrics_handle.render())))
        .route_layer(middleware::from_fn(track_metrics));

    let addr = config.http_address();
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!("fmcd listening on {addr}");
    axum::serve(listener, app).await?;
    Ok(())
}

fn setup_metrics_recorder() -> anyhow::Result<PrometheusHandle> {
    const EXPONENTIAL_SECONDS: &[f64] = &[
        0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
    ];

    Ok(PrometheusBuilder::new()
        .set_buckets_for_metric(
            Matcher::Full("http_requests_duration_seconds".to_string()),
            EXPONENTIAL_SECONDS,
        )?
        .install_recorder()?)
}

async fn track_metrics(req: Request, next: Next) -> impl IntoResponse {
    let start = Instant::now();
    let path = if let Some(matched_path) = req.extensions().get::<MatchedPath>() {
        matched_path.as_str().to_owned()
    } else {
        req.uri().path().to_owned()
    };
    let method = req.method().clone();

    let response = next.run(req).await;

    let duration = start.elapsed();
    let status_code = response.status().as_u16();

    // Use our comprehensive API metrics recording
    api_metrics::record_api_request(&method.to_string(), &path, status_code, duration);

    response
}

/// Implements Fedimint V0.2 API Route matching against CLI commands:
/// - `/v2/admin/backup`: Upload the (encrypted) snapshot of mint notes to
///   federation.
/// - `/v2/admin/discover-version`: Discover the common api version to use to
///   communicate with the federation.
/// - `/v2/admin/info`: Display wallet info (holdings, tiers).
/// - `/v2/admin/join`: Join a federation with an invite code.
/// - `/v2/admin/restore`: Restore the previously created backup of mint notes
///   (with `backup` command).
/// - `/v2/admin/list-operations`: List operations.
/// - `/v2/admin/module`: Call a module subcommand.
/// - `/v2/admin/config`: Returns the client config.
///
/// Mint related commands:
/// - `/v2/mint/reissue`: Reissue notes received from a third party to avoid
///   double spends.
/// - `/v2/mint/spend`: Prepare notes to send to a third party as a payment.
/// - `/v2/mint/validate`: Verifies the signatures of e-cash notes, but *not* if
///   they have been spent already.
/// - `/v2/mint/split`: Splits a string containing multiple e-cash notes (e.g.
///   from the `spend` command) into ones that contain exactly one.
/// - `/v2/mint/combine`: Combines two or more serialized e-cash notes strings.
///
/// Lightning network related commands:
/// - `/v2/ln/invoice`: Create a lightning invoice to receive payment via
///   gateway.
/// - `/v2/ln/invoice-external-pubkey-tweaked`: Create a lightning invoice to
///   receive payment via gateway with external pubkey.
/// - `/v2/ln/await-invoice`: Wait for incoming invoice to be paid.
/// - `/v2/ln/claim-external-receive-tweaked`: Claim an external receive.
/// - `/v2/ln/pay`: Pay a lightning invoice or lnurl via a gateway.
/// - `/v2/ln/await-pay`: Wait for a lightning payment to complete.
/// - `/v2/ln/list-gateways`: List registered gateways.
/// - `/v2/ln/switch-gateway`: Switch active gateway.
///
/// Onchain related commands:
/// - `/v2/onchain/deposit-address`: Generate a new deposit address, funds sent
///   to it can later be claimed.
/// - `/v2/onchain/await-deposit`: Wait for deposit on previously generated
///   address.
/// - `/v2/onchain/withdraw`: Withdraw funds from the federation.
fn fedimint_v2_rest() -> Router<AppState> {
    let mint_router = Router::new()
        .route("/decode-notes", post(mint::decode_notes::handle_rest))
        .route("/encode-notes", post(mint::encode_notes::handle_rest))
        .route("/reissue", post(mint::reissue::handle_rest))
        .route("/spend", post(mint::spend::handle_rest))
        .route("/validate", post(mint::validate::handle_rest))
        .route("/split", post(mint::split::handle_rest))
        .route("/combine", post(mint::combine::handle_rest));

    let ln_router = Router::new()
        .route("/invoice", post(ln::invoice::handle_rest))
        .route(
            "/invoice-external-pubkey-tweaked",
            post(ln::invoice_external_pubkey_tweaked::handle_rest),
        )
        .route("/await-invoice", post(ln::await_invoice::handle_rest))
        .route(
            "/claim-external-receive-tweaked",
            post(ln::claim_external_receive_tweaked::handle_rest),
        )
        .route("/pay", post(ln::pay::handle_rest))
        .route("/list-gateways", post(ln::list_gateways::handle_rest));

    let onchain_router = Router::new()
        .route(
            "/deposit-address",
            post(onchain::deposit_address::handle_rest),
        )
        .route("/await-deposit", post(onchain::await_deposit::handle_rest))
        .route("/withdraw", post(onchain::withdraw::handle_rest));

    let admin_router = Router::new()
        .route("/backup", post(admin::backup::handle_rest))
        .route(
            "/discover-version",
            post(admin::discover_version::handle_rest),
        )
        .route("/federation-ids", get(admin::federation_ids::handle_rest))
        .route("/info", get(admin::info::handle_rest))
        .route("/join", post(admin::join::handle_rest))
        .route("/restore", post(admin::restore::handle_rest))
        // .route("/printsecret", get(handle_printsecret)) TODO: should I expose this
        // under admin?
        .route(
            "/list-operations",
            post(admin::list_operations::handle_rest),
        )
        .route("/module", post(admin::module::handle_rest))
        .route("/config", get(admin::config::handle_rest));

    Router::new()
        .nest("/admin", admin_router)
        .nest("/mint", mint_router)
        .nest("/ln", ln_router)
        .nest("/onchain", onchain_router)
}
