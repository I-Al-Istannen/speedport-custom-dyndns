use super::{DnsEntry, DnsProvider, DnsRecordType, Origin, RecordId};
use crate::types::ensure_env_vars;
use async_trait::async_trait;
use derive_more::Display;
use jiff::{Span, Timestamp};
use rootcause::option_ext::OptionExt;
use rootcause::prelude::ResultExt;
use rootcause::{Report, report};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::sync::{Arc, Mutex};
use tracing::{debug, info, warn};

const NETCUP_ENDPOINT: &str = "https://ccp.netcup.net/run/webservice/servers/endpoint.php?JSON";

#[derive(Debug)]
enum NetcupAction {
    Login,
    InfoDnsRecords,
    UpdateDnsRecords,
}

impl Display for NetcupAction {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Login => write!(f, "login"),
            Self::InfoDnsRecords => write!(f, "infoDnsRecords"),
            Self::UpdateDnsRecords => write!(f, "updateDnsRecords"),
        }
    }
}

#[derive(Debug, Deserialize, Display, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum NetcupStatus {
    Error,
    Started,
    Pending,
    Warning,
    Success,
}

#[derive(Debug)]
struct NetcupSessionId {
    valid_until: Timestamp,
    key: String,
}

impl NetcupSessionId {
    fn new(key: String) -> Self {
        Self {
            // https://www.netcup.com/de/helpcenter/dokumentation/domain/unsere-api#authentifizierung-an-der-api
            valid_until: Timestamp::now() + Span::default().minutes(10),
            key,
        }
    }

    fn is_valid(&self) -> bool {
        self.valid_until >= Timestamp::now()
    }
}

pub struct NetcupProvider {
    api_key: String,
    api_password: String,
    api_session_id: Arc<Mutex<Option<NetcupSessionId>>>,
    customer_number: String,
    client: reqwest::Client,
}

impl NetcupProvider {
    pub fn new_from_env() -> Result<Self, Report> {
        ensure_env_vars(&[
            "NETCUP_API_KEY",
            "NETCUP_API_PASSWORD",
            "NETCUP_CUSTOMER_NUMBER",
        ])?;
        let api_key = std::env::var("NETCUP_API_KEY")
            .context("NETCUP_API_KEY environment variable not set")?;
        let api_password = std::env::var("NETCUP_API_PASSWORD")
            .context("NETCUP_API_PASSWORD environment variable not set")?;
        let customer_number = std::env::var("NETCUP_CUSTOMER_NUMBER")
            .context("NETCUP_CUSTOMER_NUMBER environment variable not set")?;

        Ok(Self {
            api_key,
            api_password,
            api_session_id: Arc::new(Mutex::new(None)),
            customer_number,
            client: reqwest::Client::new(),
        })
    }

    async fn request(
        &self,
        action: NetcupAction,
        data: &[(&'static str, serde_json::Value)],
    ) -> Result<NetcupBaseResponse, Report> {
        let mut data = data.iter().cloned().collect::<HashMap<_, _>>();
        data.insert("apikey", self.api_key.clone().into());
        data.insert("customernumber", self.customer_number.clone().into());
        if let Some(id) = &self.api_session_id.lock().expect("mutex poisoned").as_ref() {
            debug!(
                ?id,
                valid = id.is_valid(),
                "Using existing session id for Netcup API request"
            );
            data.insert("apisessionid", id.key.clone().into());
        }

        let resp = self
            .client
            .post(NETCUP_ENDPOINT)
            .json(&json!({
                "action": action.to_string(),
                "param": data
            }))
            .send()
            .await
            .context("failed to send request to Netcup")
            .attach(format!("action: '{action}'"))?
            .bytes()
            .await
            .context("failed to read body")?;

        let resp: NetcupBaseResponse = serde_json::from_slice(&resp)
            .context("failed to parse response from Netcup")
            .attach(format!("action: '{action}'"))
            .attach(format!("data: {}", String::from_utf8_lossy(resp.as_ref())))
            .map_err(Report::into_dynamic)?;

        if resp.status != NetcupStatus::Success {
            return Err(report!("Netcup API returned error")
                .attach(format!("action: '{action}'"))
                .attach(format!("status: '{}'", resp.status))
                .attach(format!("statuscode: {}", resp.statuscode))
                .attach(format!("shortmessage: '{}'", resp.shortmessage))
                .attach(format!("longmessage: '{}'", resp.longmessage)));
        }

        Ok(resp)
    }

    async fn login(&self) -> Result<NetcupSessionId, Report> {
        let resp = self
            .request(
                NetcupAction::Login,
                &[("apipassword", self.api_password.clone().into())],
            )
            .await
            .context("failed to send login to Netcup")?;
        let api_session_id = resp
            .responsedata
            .as_object()
            .context("found no response data object")?
            .get("apisessionid")
            .context("found no apisessionid in response data")?
            .as_str()
            .context("apisessionid is not a string")?;

        Ok(NetcupSessionId::new(api_session_id.to_string()))
    }

    async fn ensure_logged_in(&self) -> Result<(), Report> {
        if let Some(id) = &self.api_session_id.lock().expect("mutex poisoned").as_ref()
            && id.is_valid()
        {
            debug!(
                ?id,
                "Already logged in to Netcup with valid session id, skipping login"
            );
            return Ok(());
        }
        debug!("Logging in again");
        let new_session_id = self.login().await?;
        debug!(?new_session_id, "Got new session id from Netcup");
        *self.api_session_id.lock().expect("mutex poisoned") = Some(new_session_id);
        Ok(())
    }

    async fn list_records_netcup(&self, origin: &Origin) -> Result<Vec<NetcupDnsRecord>, Report> {
        self.ensure_logged_in().await?;

        let resp = self
            .request(
                NetcupAction::InfoDnsRecords,
                &[("domainname", origin.0.to_string().into())],
            )
            .await
            .context("failed to list records")
            .attach(format!("origin: '{origin}'"))?;

        let dns_records = resp
            .responsedata
            .as_object()
            .context("found no response data object")?
            .get("dnsrecords")
            .context("found no dnsrecords in response data")?;

        Ok(serde_json::from_value(dns_records.clone())
            .context("failed to parse dnsrecords from response data")?)
    }
}

#[async_trait]
impl DnsProvider for NetcupProvider {
    fn name(&self) -> &'static str {
        "netcup"
    }

    async fn list_records(&self, origin: &Origin) -> Result<Vec<DnsEntry>, Report> {
        Ok(self
            .list_records_netcup(origin)
            .await?
            .into_iter()
            .filter_map(|it| it.into_entry(origin))
            .collect::<Vec<_>>())
    }

    async fn update_record(
        &self,
        origin: &Origin,
        record_id: &RecordId,
        new_content: &str,
    ) -> Result<(), Report> {
        self.ensure_logged_in().await?;
        let mut patched_record = self
            .list_records_netcup(origin)
            .await?
            .into_iter()
            .find(|it| it.id == record_id.0)
            .context("record not found")
            .attach(format!("origin: '{origin}'"))
            .attach(format!("record_id: '{record_id}'"))?;
        patched_record.destination = new_content.to_string();
        patched_record.deleterecord = false;

        self.request(
            NetcupAction::UpdateDnsRecords,
            &[
                ("domainname", origin.0.to_string().into()),
                ("dnsrecordset", json!({ "dnsrecords": [patched_record]})),
            ],
        )
        .await
        .context("failed to update DNS record for origin")
        .attach(format!("origin: '{origin}'"))
        .map_err(Report::into_dynamic)
        .map(|_| ())
    }

    async fn validate(&self, origin: &Origin) -> Result<(), Report> {
        info!("Listing all DNS records...");
        let records = self
            .list_records(origin)
            .await
            .context("failed to list all DNS records")
            .attach("I think you probably want to fix that before I start...")
            .attach(format!("origin: '{origin}'"))?;

        info!("Found {} DNS records", records.len());

        Ok(())
    }
}

#[derive(Debug, Deserialize)]
struct NetcupBaseResponse {
    status: NetcupStatus,
    statuscode: u32,
    shortmessage: String,
    longmessage: String,
    responsedata: serde_json::Value,
}

#[derive(Debug, Deserialize, Serialize)]
struct NetcupDnsRecord {
    deleterecord: bool,
    destination: String,
    hostname: String,
    id: String,
    state: String,
    #[serde(rename = "type")]
    typ: String,
}

impl NetcupDnsRecord {
    fn into_entry(self, origin: &Origin) -> Option<DnsEntry> {
        let Ok(typ) = DnsRecordType::try_from(self.typ) else {
            return None;
        };
        if self.deleterecord {
            return None;
        }
        if self.state != "yes" {
            warn!(
                "Got record in unsuccessful state: {} ({} / {}.{})",
                self.state, self.id, self.hostname, origin
            );
        }
        Some(DnsEntry {
            typ,
            id: RecordId(self.id),
            content: self.destination,
            name: format!("{}.{}", self.hostname, origin.0),
        })
    }
}
