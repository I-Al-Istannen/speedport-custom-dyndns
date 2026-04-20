use async_trait::async_trait;
use derive_more::Display;
use rootcause::Report;
use tracing::debug;

pub mod cloudflare;
pub mod netcup;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Display)]
pub enum DnsRecordType {
    A,
    #[allow(clippy::upper_case_acronyms)]
    AAAA,
}

impl TryFrom<String> for DnsRecordType {
    type Error = ();

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Ok(match value.as_str() {
            "A" => Self::A,
            "AAAA" => Self::AAAA,
            _ => {
                debug!(typ = %value, "Skipping unsupported record type");
                return Err(());
            }
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Display)]
pub struct RecordId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Display)]
pub struct Origin(pub String);

impl Origin {
    pub fn is_subdomain(&self, domain: &str) -> bool {
        domain.ends_with(&format!(".{}", self.0))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DnsEntry {
    pub typ: DnsRecordType,
    pub id: RecordId,
    pub name: String,
    pub content: String,
}

#[async_trait]
pub trait DnsProvider {
    fn name(&self) -> &'static str;

    async fn list_records(&self, origin: &Origin) -> Result<Vec<DnsEntry>, Report>;
    async fn update_record(
        &self,
        origin: &Origin,
        record_id: &RecordId,
        new_content: &str,
    ) -> Result<(), Report>;

    async fn validate(&self, origin: &Origin) -> Result<(), Report>;
}
