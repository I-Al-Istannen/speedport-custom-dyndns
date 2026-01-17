use async_trait::async_trait;
use derive_more::Display;
use rootcause::Report;

pub mod cloudflare;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DnsRecordType {
    A,
    #[allow(clippy::upper_case_acronyms)]
    AAAA,
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
    async fn list_records(&self, origin: &Origin) -> Result<Vec<DnsEntry>, Report>;
    async fn update_record(
        &self,
        origin: &Origin,
        record_id: &RecordId,
        new_content: &str,
    ) -> Result<(), Report>;
}
