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
use rootcause::{bail, report};
use tokio::select;
use tokio::signal::unix::SignalKind;
use tokio::signal::unix::signal;
use tracing::Span;
use tracing::error;
use tracing::info;
use tracing::warn;
use tracing::{Instrument, debug};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::provider::DnsProvider;
use crate::provider::netcup::NetcupProvider;
use crate::types::ensure_env_vars;
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

    ensure_env_vars(&["PASSWORD", "ORIGIN", "PROVIDERS"])?;

    let interface = std::env::var("INTERFACE").unwrap_or("0.0.0.0".to_string());
    let port: String = std::env::var("PORT").unwrap_or("3000".to_string());
    let client_password =
        std::env::var("PASSWORD").context("PASSWORD environment variable not set")?;
    let origin_str = std::env::var("ORIGIN").context("ORIGIN environment variable not set")?;
    let enabled_providers =
        std::env::var("PROVIDERS").context("PROVIDERS environment variable not set")?;

    let mut dns_providers: Vec<Arc<dyn DnsProvider + Send + Sync>> = Vec::new();
    for provider in enabled_providers.split(",").filter(|s| !s.is_empty()) {
        match provider.trim().to_ascii_lowercase().as_str() {
            "cloudflare" => dns_providers.push(Arc::new(CloudflareProvider::new_from_env()?)),
            "netcup" => dns_providers.push(Arc::new(NetcupProvider::new_from_env()?)),
            other => {
                bail!(
                    "Unknown provider specified in PROVIDERS environment variable: '{}'",
                    other
                );
            }
        }
    }

    if dns_providers.is_empty() {
        return Err(report!("No valid providers found")
            .attach(format!("env PROVIDERS={enabled_providers}")));
    }

    let state = AppState {
        client_password,
        dns_origin: Origin(origin_str),
        dns_providers,
    };

    for provider in &state.dns_providers {
        provider
            .validate(&state.dns_origin)
            .await
            .context("Failed to validate DNS provider")
            .attach(format!("Provider: {}", provider.name()))?;
    }

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
