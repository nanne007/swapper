use crate::{execution::balanceOfCall, rpc::map_rpc_error};
use alloy_primitives::{Address, B256, U256, keccak256};
use alloy_provider::{DynProvider, Provider};
use alloy_rpc_types_eth::{BlockId, TransactionRequest};
use alloy_sol_types::SolCall;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tokio::sync::Mutex as AsyncMutex;

/// Configured Solidity mapping base slots, indexed by chain and token.
pub type BalanceSlotConfig = HashMap<u64, HashMap<Address, U256>>;

const CACHE_CAPACITY: usize = 1024;
const MAX_CANDIDATES: usize = 32;
const AUTO_BASE_SLOTS: u64 = 1024;

#[derive(Debug, thiserror::Error)]
pub enum BalanceSlotError {
    #[error("BALANCE_SLOT_NOT_FOUND")]
    NotFound,
    #[error("BALANCE_SLOT_AMBIGUOUS")]
    Ambiguous,
    #[error("BALANCE_SLOT_PROBE_LIMIT")]
    ProbeLimit,
    #[error("BALANCE_SLOT_CACHE_BUSY")]
    CacheBusy,
}

type SharedBase = Arc<AsyncMutex<Option<U256>>>;

/// Server-owned mapping bases, reusable across owners of the same token on the same chain.
pub struct BalanceSlots {
    configured: BalanceSlotConfig,
    cache: Mutex<HashMap<(u64, Address), SharedBase>>,
}

impl BalanceSlots {
    pub fn new(configured: BalanceSlotConfig) -> Self {
        Self {
            configured,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Resolve a mapping base without a wallet, amount, or simulation funding assumptions.
    pub async fn resolve(
        &self,
        rpc: &DynProvider,
        chain_id: u64,
        token: Address,
        block: BlockId,
    ) -> anyhow::Result<U256> {
        if let Some(base) = self
            .configured
            .get(&chain_id)
            .and_then(|slots| slots.get(&token))
        {
            return Ok(*base);
        }
        let entry = self.entry((chain_id, token))?;
        // Concurrent callers share discovery; cancellation releases this guard.
        let mut cached = entry.lock().await;
        if let Some(base) = *cached {
            return Ok(base);
        }
        let first = probe_address(b"MetaMatch balance mapping probe A");
        let second = probe_address(b"MetaMatch balance mapping probe B");
        let (mut candidates, other) = tokio::try_join!(
            access_list_bases(rpc, token, first, block),
            access_list_bases(rpc, token, second, block),
        )?;
        candidates.retain(|base| other.contains(base));
        let base = match candidates.as_slice() {
            [base] => *base,
            [] => anyhow::bail!(BalanceSlotError::NotFound),
            _ => anyhow::bail!(BalanceSlotError::Ambiguous),
        };
        *cached = Some(base);
        Ok(base)
    }

    fn entry(&self, key: (u64, Address)) -> anyhow::Result<SharedBase> {
        let mut cache = self.cache.lock().expect("balance slot cache lock poisoned");
        if let Some(entry) = cache.get(&key) {
            return Ok(entry.clone());
        }
        if cache.len() == CACHE_CAPACITY {
            // Eviction order is irrelevant; protect active discoveries and waiting callers.
            let idle = cache
                .iter()
                .find_map(|(key, base)| (Arc::strong_count(base) == 1).then_some(*key))
                .ok_or(BalanceSlotError::CacheBusy)?;
            cache.remove(&idle);
        }
        let base = Arc::new(AsyncMutex::new(None));
        cache.insert(key, base.clone());
        Ok(base)
    }
}

fn balance_call(token: Address, owner: Address) -> TransactionRequest {
    TransactionRequest::default()
        .to(token)
        .input(balanceOfCall { owner }.abi_encode().into())
        .gas_limit(100_000)
}

/// Solidity mapping entry location; no RPC or state mutation.
pub fn storage_key(owner: Address, base: U256) -> B256 {
    let mut encoded = [0u8; 64];
    encoded[12..32].copy_from_slice(owner.as_slice());
    encoded[32..].copy_from_slice(&base.to_be_bytes::<32>());
    keccak256(encoded)
}

fn probe_address(label: &[u8]) -> Address {
    Address::from_slice(&keccak256(label)[12..])
}

async fn access_list_bases(
    rpc: &DynProvider,
    token: Address,
    probe: Address,
    block: BlockId,
) -> anyhow::Result<Vec<U256>> {
    let result = rpc
        .create_access_list(&balance_call(token, probe))
        .block_id(block)
        .await
        .map_err(|error| map_rpc_error("eth_createAccessList", error))?;
    if let Some(error) = result.error {
        return Err(anyhow::Error::msg(error).context(crate::error::ErrorKind::RpcCallFailed));
    }
    let Some(account) = result
        .access_list
        .iter()
        .find(|account| account.address == token)
    else {
        return Ok(Vec::new());
    };
    if account.storage_keys.len() > MAX_CANDIDATES {
        anyhow::bail!(BalanceSlotError::ProbeLimit);
    }
    // Only infer bases whose derived keys appear in the token's generated access list.
    // Two probes disambiguate fixed/unrelated keys without state overrides.
    Ok((0..AUTO_BASE_SLOTS)
        .map(U256::from)
        .filter(|base| account.storage_keys.contains(&storage_key(probe, *base)))
        .collect())
}
