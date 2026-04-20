use super::{DnsEntry, DnsProvider, DnsRecordType, Origin, RecordId};
use crate::types::ensure_env_vars;
use async_trait::async_trait;
use rootcause::prelude::ResultExt;
use rootcause::{Report, report};
use serde_json::json;
use tracing::info;

pub struct CloudflareProvider {
    api_token: String,
    client: reqwest::Client,
}

impl CloudflareProvider {
    pub fn new_from_env() -> Result<Self, Report> {
        ensure_env_vars(&["CLOUDFLARE_API_TOKEN"])?;
        let api_token = std::env::var("CLOUDFLARE_API_TOKEN")
            .context("CLOUDFLARE_API_TOKEN environment variable not set")?;

        Ok(Self {
            api_token,
            client: reqwest::Client::new(),
        })
    }

    async fn get_zone_id(&self, origin: &Origin) -> Result<String, Report> {
        let response = self
            .client
            .get("https://api.cloudflare.com/client/v4/zones")
            .bearer_auth(&self.api_token)
            .query(&[("domain", &origin.0)])
            .send()
            .await
            .context("Listing records")
            .attach(format!("origin: '{origin}'"))?;

        let response = response
            .json::<CloudflareZoneResponse>()
            .await
            .context("Parsing Cloudflare zones response")
            .attach(format!("origin: '{origin}'"))?;

        if response.result.len() > 1 {
            return Err(report!("Multiple zones found for origin")
                .attach(format!("origin: '{origin}'"))
                .attach(format!(
                    "zones: {:?}",
                    response.result.iter().map(|z| &z.name).collect::<Vec<_>>()
                )));
        }

        let Some(zone_result) = response.result.into_iter().find(|it| it.name == origin.0) else {
            return Err(report!("No zone found for origin").attach(format!("origin: '{origin}'")));
        };

        Ok(zone_result.id.clone())
    }
}

#[async_trait]
impl DnsProvider for CloudflareProvider {
    fn name(&self) -> &'static str {
        "Cloudflare"
    }

    async fn list_records(&self, origin: &Origin) -> Result<Vec<DnsEntry>, Report> {
        let zone_id = self.get_zone_id(origin).await?;
        let response = self
            .client
            .get(format!(
                "https://api.cloudflare.com/client/v4/zones/{}/dns_records",
                zone_id
            ))
            .bearer_auth(&self.api_token)
            .query(&[("per_page", "10000")])
            .send()
            .await
            .context("Listing DNS records from Cloudflare")
            .attach(format!("origin: '{origin}'"))?;

        if !response.status().is_success() {
            return Err(report!("Failed to list DNS records from Cloudflare")
                .attach(format!("origin: '{origin}'"))
                .attach(format!("status: {}", response.status()))
                .attach(format!(
                    "response: {:?}",
                    response
                        .text()
                        .await
                        .unwrap_or("<Response reading failed>".to_string())
                )));
        }

        let response = response
            .json::<CloudflareListRecordsResponse>()
            .await
            .context("Parsing Cloudflare DNS records response")
            .attach(format!("origin: '{origin}'"))?;

        Ok(response
            .result
            .into_iter()
            .filter_map(|it| it.into())
            .collect())
    }

    async fn update_record(
        &self,
        origin: &Origin,
        record_id: &RecordId,
        new_content: &str,
    ) -> Result<(), Report> {
        let zone_id = self.get_zone_id(origin).await?;
        let response = self
            .client
            .patch(format!(
                "https://api.cloudflare.com/client/v4/zones/{}/dns_records/{}",
                zone_id, record_id.0
            ))
            .bearer_auth(&self.api_token)
            .json(&json!({
                "content": new_content
            }))
            .send()
            .await
            .context("Updating DNS record in Cloudflare")
            .attach(format!("origin: '{origin}'"))
            .attach(format!("record_id: '{record_id}'"))?;

        if response.status().is_success() {
            Ok(())
        } else {
            Err(report!("Failed to update DNS record in Cloudflare")
                .attach(format!("origin: '{origin}'"))
                .attach(format!("record_id: '{record_id}'"))
                .attach(format!("status: {}", response.status()))
                .attach(format!(
                    "response: {:?}",
                    response
                        .text()
                        .await
                        .unwrap_or("<Response reading failed>".to_string())
                )))
        }
    }

    async fn validate(&self, origin: &Origin) -> Result<(), Report> {
        info!("Listing all DNS records...");
        let zone_dns_records = self
            .list_records(origin)
            .await
            .context("Failed to list DNS records on startup")
            .attach("I think you probably want to fix that before I start...")
            .attach(format!("Origin: {}", origin.0))?;

        info!("Found {} DNS records", zone_dns_records.len());

        Ok(())
    }
}

#[derive(serde::Deserialize, Clone)]
struct CloudflareZoneResponse {
    result: Vec<CloudflareZone>,
}

#[derive(serde::Deserialize, Clone)]
struct CloudflareZone {
    id: String,
    name: String,
}

#[derive(serde::Deserialize, Clone)]
struct CloudflareListRecordsResponse {
    result: Vec<CloudflareDnsRecord>,
}

#[derive(serde::Deserialize, Clone)]
struct CloudflareDnsRecord {
    id: String,
    r#type: String,
    name: String,
    content: String,
}

impl From<CloudflareDnsRecord> for Option<DnsEntry> {
    fn from(record: CloudflareDnsRecord) -> Self {
        let Ok(typ) = DnsRecordType::try_from(record.r#type) else {
            return None;
        };
        Some(DnsEntry {
            typ,
            id: RecordId(record.id),
            name: record.name,
            content: record.content,
        })
    }
}
