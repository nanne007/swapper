use crate::error::ErrorKind;
use crate::{
    balance_slots::{BalanceSlotError, BalanceSlots, storage_key},
    domain::{Address, BlockContext, Chain, Input, NATIVE, SimulationSuccess, Tx, parse_uint},
    execution::{ValidatedRoute, approval, balance_data, swap_transaction},
    rpc::{RpcClients, map_rpc_error},
};
use alloy_primitives::{B256, Bytes, U256};
use alloy_provider::{DynProvider, Provider};
use alloy_rpc_types_eth::{
    BlockId, TransactionInput, TransactionRequest,
    simulate::{SimBlock, SimCallResult, SimulatePayload, SimulatedBlock},
    state::StateOverride,
};
use anyhow::Context as _;
use async_trait::async_trait;
use std::sync::Arc;

#[derive(Debug)]
pub struct SimResult {
    pub simulation: SimulationSuccess,
    pub approvals: Vec<Tx>,
    pub transaction: Tx,
}

/// Preserve Alloy's per-call error and revert bytes in the anyhow source chain.
#[derive(Debug, thiserror::Error)]
#[error("eth_simulateV1 call {index} failed: {result:?}")]
pub struct SimulationCallError {
    pub index: usize,
    pub result: SimCallResult,
}

pub struct SimulationRequest<'a> {
    pub input: &'a Input,
    pub chain: &'a Chain,
    pub route: &'a ValidatedRoute,
}

#[async_trait]
pub trait SimulationProvider: Send + Sync {
    async fn simulate(&self, request: SimulationRequest<'_>) -> anyhow::Result<SimResult>;
}

pub struct Simulator {
    clients: Arc<RpcClients>,
    balance_slots: Arc<BalanceSlots>,
}

impl Simulator {
    pub fn new(clients: Arc<RpcClients>, balance_slots: Arc<BalanceSlots>) -> Self {
        Self {
            clients,
            balance_slots,
        }
    }

    pub async fn simulate(&self, request: SimulationRequest<'_>) -> anyhow::Result<SimResult> {
        let SimulationRequest {
            input,
            chain,
            route,
        } = request;
        let router = chain.router.context(ErrorKind::RouterNotConfigured)?;
        let rpc = self.rpc(chain)?;
        let block = BlockId::latest();
        let (sell_balance_slot, approvals) = if input.sell_token == NATIVE {
            (None, Vec::new())
        } else {
            let base = self
                .balance_slots
                .resolve(&rpc, chain.id, input.sell_token, block)
                .await
                .map_err(balance_slot_failure)?;
            let sell_amount = parse_uint(&input.sell_amount)?;
            (
                Some(storage_key(input.taker, base)),
                vec![approval(input.sell_token, router.holder, sell_amount)],
            )
        };
        let transaction = swap_transaction(input, router, route)?;
        let taker = input.taker;
        let mut overrides = match sell_balance_slot {
            Some(slot) => {
                token_balance_override(input.sell_token, slot, parse_uint(&input.sell_amount)?)
            }
            None => StateOverride::default(),
        };
        let balance_call = contract_call(input.buy_token, balance_data(taker))
            .from(taker)
            .gas_limit(100_000);
        let mut calls = vec![balance_call.clone()];
        calls.extend(
            approvals
                .iter()
                .map(|approval| rpc_transaction(approval, taker))
                .collect::<Result<Vec<_>, _>>()?,
        );
        calls.push(rpc_transaction(&transaction, taker)?);
        calls.push(balance_call);
        // The node chooses the latest block fee while no gasPrice is supplied. A maximal
        // simulation-only balance avoids an extra fee RPC and upfront gas-limit failures.
        overrides.entry(taker).or_default().set_balance(U256::MAX);
        let payload = SimulatePayload {
            block_state_calls: vec![SimBlock {
                block_overrides: None,
                state_overrides: Some(overrides),
                calls,
            }],
            validation: false,
            trace_transfers: false,
            return_full_transactions: false,
        };
        let raw = rpc
            .simulate(&payload)
            .latest()
            .await
            .map_err(|error| rpc_failure(map_rpc_error("eth_simulateV1", error)))?;
        let (block_context, call_results) = parse_simulation_block(raw)?;
        if call_results.len() != approvals.len() + 3 {
            anyhow::bail!(SimulationFailure::Error("SIMULATION_CALL_COUNT_MISMATCH"));
        }
        if let Some((index, call)) = call_results
            .iter()
            .enumerate()
            .find(|(_, call)| !call.status)
        {
            return Err(anyhow::Error::new(SimulationCallError {
                index,
                result: call.clone(),
            })
            .context(ErrorKind::SimulationReverted)
            .context(SimulationFailure::Reverted("SIMULATION_REVERTED")));
        }
        if call_results[1..1 + approvals.len()]
            .iter()
            .any(|call| !call.return_data.is_empty() && call.return_data != word_bytes(U256::ONE))
        {
            anyhow::bail!(SimulationFailure::Reverted("APPROVAL_RETURNED_FALSE"));
        }
        let before = parse_return_word(
            &call_results
                .first()
                .expect("call count was checked")
                .return_data,
        )?;
        let after = parse_return_word(
            &call_results
                .last()
                .expect("call count was checked")
                .return_data,
        )?;
        if after < before {
            anyhow::bail!(SimulationFailure::Reverted(
                "SIMULATION_INVALID_BALANCE_DELTA"
            ));
        }
        let bought = after - before;
        if bought < parse_uint(&route.min_buy_amount)? {
            anyhow::bail!(SimulationFailure::Reverted(
                "SIMULATED_OUTPUT_BELOW_MINIMUM"
            ));
        }
        let gas_used = call_results[1..call_results.len() - 1]
            .iter()
            .fold(U256::ZERO, |sum, call| sum + U256::from(call.gas_used));
        let simulated_timestamp = block_context.timestamp;
        Ok(SimResult {
            simulation: SimulationSuccess {
                bought_amount: bought.to_string(),
                gas_used: gas_used.to_string(),
                gas_fee_wei: None,
                funding: "overridden".into(),
                block_context,
                simulated_timestamp,
            },
            approvals,
            transaction,
        })
    }

    fn rpc(&self, chain: &Chain) -> anyhow::Result<DynProvider> {
        let Some(url) = &chain.rpc_url else {
            anyhow::bail!(SimulationFailure::Unsupported("RPC_NOT_CONFIGURED"));
        };
        self.clients.get(url)
    }
}

#[async_trait]
impl SimulationProvider for Simulator {
    async fn simulate(&self, request: SimulationRequest<'_>) -> anyhow::Result<SimResult> {
        Simulator::simulate(self, request).await
    }
}

/// Typed outcome context, not an error container: anyhow retains the original source.
#[derive(Debug, Clone, Copy, thiserror::Error, serde::Serialize)]
#[serde(tag = "status", content = "reason", rename_all = "camelCase")]
pub enum SimulationFailure {
    #[error("{0}")]
    Reverted(&'static str),
    #[error("{0}")]
    Unsupported(&'static str),
    #[error("{0}")]
    Error(&'static str),
}

fn rpc_failure(error: anyhow::Error) -> anyhow::Error {
    let outcome = match crate::error::kind(&error) {
        ErrorKind::RpcMethodUnsupported => SimulationFailure::Unsupported("RPC_METHOD_UNSUPPORTED"),
        ErrorKind::RpcInvalidResponse => SimulationFailure::Error("RPC_INVALID_RESPONSE"),
        _ => SimulationFailure::Error("RPC_CALL_FAILED"),
    };
    error.context(outcome)
}

fn contract_call(to: Address, data: Bytes) -> TransactionRequest {
    TransactionRequest::default()
        .to(to)
        .input(TransactionInput::new(data))
}

fn rpc_transaction(transaction: &Tx, from: Address) -> anyhow::Result<TransactionRequest> {
    Ok(TransactionRequest::default()
        .from(from)
        .to(transaction.to)
        .input(TransactionInput::new(transaction.data.clone()))
        .value(parse_uint(&transaction.value)?)
        .gas_limit(0x7a1200))
}

fn parse_return_word(value: &Bytes) -> anyhow::Result<U256> {
    if value.len() != 32 {
        anyhow::bail!(ErrorKind::RpcInvalidResponse);
    }
    Ok(U256::from_be_slice(value))
}

fn word_bytes(value: U256) -> Bytes {
    Bytes::from(value.to_be_bytes::<32>())
}

fn parse_simulation_block(
    blocks: Vec<SimulatedBlock>,
) -> anyhow::Result<(BlockContext, Vec<SimCallResult>)> {
    if blocks.len() != 1 {
        anyhow::bail!(ErrorKind::RpcInvalidResponse);
    }
    let block = blocks.into_iter().next().expect("block count was checked");
    Ok((
        BlockContext {
            number: block.inner.header.inner.number,
            hash: format!("{:#x}", block.inner.header.hash),
            timestamp: block.inner.header.inner.timestamp,
        },
        block.calls,
    ))
}

fn token_balance_override(token: Address, slot: B256, amount: U256) -> StateOverride {
    let mut overrides = StateOverride::default();
    overrides
        .entry(token)
        .or_default()
        .set_state_diff([(slot, B256::from(amount.to_be_bytes::<32>()))]);
    overrides
}

fn balance_slot_failure(error: anyhow::Error) -> anyhow::Error {
    let reason = match error.downcast_ref::<BalanceSlotError>() {
        Some(BalanceSlotError::NotFound) => "BALANCE_OVERRIDE_SLOT_NOT_FOUND",
        Some(BalanceSlotError::Ambiguous) => "BALANCE_OVERRIDE_SLOT_AMBIGUOUS",
        Some(BalanceSlotError::ProbeLimit) => "BALANCE_OVERRIDE_PROBE_LIMIT",
        Some(BalanceSlotError::CacheBusy) => "BALANCE_OVERRIDE_CACHE_BUSY",
        None => return rpc_failure(error),
    };
    error.context(SimulationFailure::Unsupported(reason))
}
