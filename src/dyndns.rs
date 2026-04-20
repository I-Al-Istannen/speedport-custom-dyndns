use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use rootcause::{Report, bail, prelude::ResultExt};
use serde::Deserialize;
use std::collections::HashSet;
use std::{
    net::{Ipv4Addr, Ipv6Addr},
    str::FromStr,
};
use tracing::{info, warn};

use crate::provider::{DnsProvider, Origin};
use crate::{provider::DnsRecordType, types::AppState};

pub(crate) async fn handle_dyndns_request(
    State(state): State<AppState>,
    Query(query): Query<UpdateQuery>,
) -> Result<String, Response> {
    info!(query = ?query, "handling update");

    let ip = ParsedIpUpdate::from_str(&query.myip).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            format!("invalid 'myip' parameter: {}", e),
        )
            .into_response()
    })?;

    info!(ip = ?ip, domain=?query.hostname, "parsed IP update");
    let mut all_ips = HashSet::new();

    for provider in &state.dns_providers {
        let expected_origin =
            state.map_origin(state.origin_for(provider.as_ref()), provider.as_ref());
        let actual_origin = state.map_origin(Origin(query.hostname.clone()), provider.as_ref());
        if !expected_origin.is_subdomain(&actual_origin.0) {
            warn!(
                query = %query.hostname,
                mapped = %actual_origin,
                expected = %expected_origin,
                "requested domain is not a subdomain of the configured origin"
            );
            return Err((
                StatusCode::BAD_REQUEST,
                format!(
                    "domain '{}' (=> '{}') is not a subdomain of '{}'",
                    query.hostname,
                    actual_origin,
                    state.origin_for(provider.as_ref())
                ),
            )
                .into_response());
        }

        let ips = match update_record(&state, provider.as_ref(), &actual_origin.0, &ip).await {
            Err(e) => {
                warn!(
                    error = %e,
                    query = %query.hostname,
                    mapped = %actual_origin,
                    ip = ?ip,
                    "failed to update DNS record"
                );
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("failed to update DNS record: {}", e),
                )
                    .into_response());
            }
            Ok(ips) => ips,
        };
        info!(
            query = %query.hostname,
            mapped = %actual_origin,
            ips = ?ips,
            provider = %provider.name(),
            "successfully updated DNS record"
        );
        all_ips.extend(ips)
    }

    Ok(all_ips
        .into_iter()
        .map(|it| format!("good {}", it))
        .collect::<Vec<_>>()
        .join("\n"))
}

async fn update_record(
    state: &AppState,
    provider: &(dyn DnsProvider + Send + Sync),
    domain: &str,
    ip: &ParsedIpUpdate,
) -> Result<Vec<String>, Report> {
    let mut updated_ips = Vec::new();

    let records = provider
        .list_records(&state.origin_for(provider))
        .await?
        .into_iter()
        .filter(|r| r.name == domain)
        .collect::<Vec<_>>();

    for (record_type, new_ip) in &ip.record_update {
        let Some(record) = records.iter().find(|it| &it.typ == record_type) else {
            info!(
                domain = %domain,
                record_type= ?record_type,
                "No existing record found, skipping update"
            );
            continue;
        };
        provider
            .update_record(&state.origin_for(provider), &record.id, new_ip)
            .await
            .attach(format!("For domain '{domain}'"))
            .attach(format!("For {:?} record", record_type))?;

        updated_ips.push(new_ip.clone());
    }

    Ok(updated_ips)
}

#[derive(Debug, Clone)]
struct ParsedIpUpdate {
    record_update: Vec<(DnsRecordType, String)>,
}

impl FromStr for ParsedIpUpdate {
    type Err = Report;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut record_update = Vec::new();

        for part in s.split(',') {
            if part.contains('.') {
                record_update.push((
                    DnsRecordType::A,
                    part.to_string()
                        .parse::<Ipv4Addr>()
                        .context("Could not parse IPv4")?
                        .to_string(),
                ));
            } else if part.contains(':') {
                record_update.push((
                    DnsRecordType::AAAA,
                    part.to_string()
                        .parse::<Ipv6Addr>()
                        .context("Could not parse IPv6")?
                        .to_string(),
                ));
            } else {
                bail!("IP part '{}' is neither IPv4 nor IPv6", part);
            }
        }

        if record_update.is_empty() {
            bail!("No IP addresses found in '{s}'");
        }

        Ok(Self { record_update })
    }
}

#[derive(Deserialize, Debug)]
pub struct UpdateQuery {
    pub myip: String,
    pub hostname: String,
}
