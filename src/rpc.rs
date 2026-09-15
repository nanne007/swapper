use crate::error::ErrorKind;
use alloy_provider::{DynProvider, Provider, ProviderBuilder};
use anyhow::Context as _;
use std::{collections::HashMap, sync::Mutex, time::Duration};

/// Reuse one Alloy provider per configured RPC URL across bootstrap and simulations.
pub struct RpcClients {
    client: reqwest::Client,
    providers: Mutex<HashMap<String, DynProvider>>,
}

impl RpcClients {
    pub fn new(timeout: Duration) -> Self {
        Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(timeout)
                .build()
                .expect("static RPC client configuration must be valid"),
            providers: Mutex::new(HashMap::new()),
        }
    }

    pub fn get(&self, url: &str) -> anyhow::Result<DynProvider> {
        let mut providers = self.providers.lock().expect("RPC clients lock poisoned");
        if let Some(provider) = providers.get(url) {
            return Ok(provider.clone());
        }
        let endpoint = url
            .parse::<reqwest::Url>()
            .context(ErrorKind::RpcNotConfigured)?;
        let provider = ProviderBuilder::default()
            .connect_reqwest(self.client.clone(), endpoint)
            .erased();
        providers.insert(url.to_owned(), provider.clone());
        Ok(provider)
    }
}

pub fn map_rpc_error(
    method: &'static str,
    error: alloy_provider::transport::TransportError,
) -> anyhow::Error {
    let code = if error
        .as_error_resp()
        .is_some_and(|response| response.code == -32601)
    {
        ErrorKind::RpcMethodUnsupported
    } else if error.is_null_resp() || error.is_deser_error() {
        ErrorKind::RpcInvalidResponse
    } else {
        ErrorKind::RpcCallFailed
    };
    anyhow::Error::new(error).context(code).context(method)
}
