use crate::provider::{DnsProvider, Origin};
use rootcause::report;
use std::collections::HashMap;
use std::env::VarError;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub dns_providers: Vec<Arc<dyn DnsProvider + Send + Sync>>,
    dns_origin: Origin,
    pub client_password: String,
    pub provider_origin_mappings: HashMap<String, Vec<(Origin, Origin)>>,
}

impl AppState {
    pub fn new(
        dns_origin: Origin,
        client_password: String,
        dns_providers: Vec<Arc<dyn DnsProvider + Send + Sync>>,
        provider_origin_mappings: HashMap<String, Vec<(Origin, Origin)>>,
    ) -> Self {
        Self {
            dns_providers,
            dns_origin,
            client_password,
            provider_origin_mappings,
        }
    }

    pub fn map_origin(&self, origin: Origin, provider: &dyn DnsProvider) -> Origin {
        let Some(mappings) = self.provider_origin_mappings.get(provider.name()) else {
            return origin;
        };
        let mut current_origin = origin.0;
        for (from, to) in mappings {
            current_origin = current_origin.replace(&from.0, &to.0);
        }

        Origin(current_origin)
    }

    pub fn origin_for(&self, provider: &dyn DnsProvider) -> Origin {
        self.map_origin(self.dns_origin.clone(), provider)
    }
}

pub fn ensure_env_vars(vars: &[&str]) -> Result<(), rootcause::Report> {
    let mut error = report!("Missing required environment variable");
    let mut is_error = false;
    for var in vars {
        match std::env::var(var) {
            Ok(_) => continue,
            Err(VarError::NotPresent) => {
                error = error.attach(format!("'{}' is not set", var));
                if *var == "PROVIDERS" {
                    error = error.attach("  valid providers: cloudflare,netcup".to_string())
                }
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
