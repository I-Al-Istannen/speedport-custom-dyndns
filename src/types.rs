use crate::provider::{DnsProvider, Origin};
use rootcause::report;
use std::env::VarError;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub dns_providers: Vec<Arc<dyn DnsProvider + Send + Sync>>,
    pub dns_origin: Origin,
    pub client_password: String,
}

pub fn ensure_env_vars(vars: &[&str]) -> Result<(), rootcause::Report> {
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
