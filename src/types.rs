use std::sync::Arc;

use crate::provider::{DnsProvider, Origin};

#[derive(Clone)]
pub struct AppState {
    pub dns_provider: Arc<dyn DnsProvider + Send + Sync>,
    pub dns_origin: Origin,
    pub client_password: String,
}
