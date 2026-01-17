use std::env::VarError;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{ConnectInfo, Request, State};
use axum::middleware::{self, Next};
use axum::response::IntoResponse;
use axum::{Router, routing::get};
use axum_extra::TypedHeader;
use axum_extra::headers::Authorization;
use axum_extra::headers::authorization::Basic;
use rootcause::prelude::ResultExt;
use rootcause::report;
use tokio::select;
use tokio::signal::unix::SignalKind;
use tokio::signal::unix::signal;
use tracing::Span;
use tracing::error;
use tracing::info;
use tracing::warn;
use tracing::{Instrument, debug};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::{
    provider::{Origin, cloudflare::CloudflareProvider},
    types::AppState,
};

mod dyndns;
mod provider;
mod types;

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    if let Err(e) = run_server().await {
        error!(err = %e, "Application error");
        std::process::exit(1);
    }
}

async fn run_server() -> Result<(), rootcause::Report> {
    info!("Starting server");

    ensure_env_vars(&["CLOUDFLARE_API_TOKEN", "PASSWORD", "ORIGIN"])?;

    let interface = std::env::var("INTERFACE").unwrap_or("0.0.0.0".to_string());
    let port: String = std::env::var("PORT").unwrap_or("3000".to_string());
    let cloudflare_token = std::env::var("CLOUDFLARE_API_TOKEN")
        .context("CLOUDFLARE_API_TOKEN environment variable not set")?;
    let client_password =
        std::env::var("PASSWORD").context("PASSWORD environment variable not set")?;
    let origin_str = std::env::var("ORIGIN").context("ORIGIN environment variable not set")?;

    let state = AppState {
        client_password,
        dns_origin: Origin(origin_str),
        dns_provider: Arc::new(CloudflareProvider::new(&cloudflare_token)),
    };

    validate_dns_zone(&state).await?;

    let app = Router::new()
        .route("/nic/update", get(dyndns::handle_dyndns_request))
        .layer(middleware::from_fn_with_state(state.clone(), ensure_auth))
        .with_state(state);

    let listen_addr = format!("{}:{}", interface, port);
    let listener = tokio::net::TcpListener::bind(&listen_addr)
        .await
        .context("Failed to bind to listen address")?;

    info!(
        "Listening on {}",
        listener.local_addr().context("Getting local address")?
    );

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async { graceful_shutdown().await }.instrument(Span::current()))
    .await
    .context("Server error")?;

    Ok(())
}

fn ensure_env_vars(vars: &[&str]) -> Result<(), rootcause::Report> {
    let mut error = report!("Missing required environment variable");
    let mut is_error = false;
    for var in vars {
        match std::env::var(var) {
            Ok(_) => continue,
            Err(VarError::NotPresent) => {
                error = error.attach(format!("'{}' is not set", var));
                is_error = true;
            }
            Err(VarError::NotUnicode(e)) => {
                error = error.attach(format!("'{}' is not valid unicode: '{}'", var, e.display()));
                is_error = true;
            }
        }
    }
    if is_error { Err(error) } else { Ok(()) }
}

async fn validate_dns_zone(state: &AppState) -> Result<(), rootcause::Report> {
    info!("Listing all DNS records...");
    let zone_dns_records = state
        .dns_provider
        .list_records(&state.dns_origin)
        .await
        .context("Failed to list DNS records on startup")
        .attach("I think you probably want to fix that before I start...")
        .attach(format!("Origin: {}", state.dns_origin.0))?;

    info!("Found {} DNS records", zone_dns_records.len());

    Ok(())
}

async fn ensure_auth(
    TypedHeader(header): TypedHeader<Authorization<Basic>>,
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    req: Request,
    next: Next,
) -> impl IntoResponse {
    // Verify basic auth
    if header.password() != state.client_password {
        let client_ip = match req.headers().get("X-Forwarded-For") {
            Some(v) => v.to_str().unwrap_or("<invalid utf8>").to_string(),
            None => addr.ip().to_string(),
        };
        debug!(
            "Invalid password attempt for user {} from ip {client_ip}",
            header.username()
        );
        return (
            axum::http::StatusCode::UNAUTHORIZED,
            "invalid password".to_string(),
        )
            .into_response();
    }
    next.run(req).await
}

async fn graceful_shutdown() {
    let mut sigterm = signal(SignalKind::terminate()).unwrap();
    let interrupt = tokio::signal::ctrl_c();
    select! {
        _ = sigterm.recv() => warn!("Received SIGTERM"),
        _ = interrupt => warn!("Received SIGINT")
    }
}
