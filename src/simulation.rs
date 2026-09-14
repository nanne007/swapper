use crate::error::ErrorKind;
use crate::{
    domain::{
        Address, Chain, Context, Input, NATIVE, Route, Rule, Simulation, Tx, parse_hex, parse_uint,
    },
    execution::{allowance_data, approval, balance_data, swap_transaction},
    rpc::{RpcClients, block_id, block_number, map_rpc_error},
};
use alloy_primitives::{B256, Bytes, U256, keccak256};
use alloy_provider::{DynProvider, Provider};
use alloy_rpc_types_eth::{
    BlockOverrides, TransactionInput, TransactionRequest,
    simulate::{SimBlock, SimCallResult, SimulatePayload, SimulatedBlock},
    state::StateOverride,
};
use anyhow::Context as _;
use async_trait::async_trait;
use std::sync::Arc;

#[derive(Debug)]
pub struct SimResult {
    pub simulation: Simulation,
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
    pub route: &'a Route,
    pub context: &'a Context,
    pub rules: &'a [Rule],
    pub taker: Address,
    pub actual: bool,
    pub min: Option<&'a str>,
}

#[async_trait]
pub trait SimulationProvider: Send + Sync {
    async fn run(&self, request: SimulationRequest<'_>) -> anyhow::Result<SimResult>;
}

pub struct Simulator {
    clients: Arc<RpcClients>,
}

impl Simulator {
    pub fn new(clients: Arc<RpcClients>) -> Self {
        Self { clients }
    }

    pub async fn run(&self, request: SimulationRequest<'_>) -> anyhow::Result<SimResult> {
        let SimulationRequest {
            input,
            chain,
            route,
            context,
            rules,
            taker,
            actual,
            min,
        } = request;
        let transaction = swap_transaction(input, chain, route, rules, min)?;
        let mut approvals = Vec::new();
        let Some(url) = &chain.rpc_url else {
            anyhow::bail!(SimulationFailure::Unsupported("RPC_NOT_CONFIGURED"));
        };
        let rpc = self.clients.get(url)?;
        assert_block(&rpc, context).await.map_err(rpc_failure)?;
        let block = block_id(context)?;
        if let Some(router) = chain.router {
            let code = rpc
                .get_code_at(router)
                .block_id(block)
                .await
                .map_err(|error| rpc_failure(map_rpc_error("eth_getCode", error)))?;
            if code.is_empty() {
                anyhow::bail!(SimulationFailure::Unsupported("ROUTER_NOT_DEPLOYED"));
            }
        }
        let mut overrides = StateOverride::default();
        let native_balance = rpc
            .get_balance(taker)
            .block_id(block)
            .await
            .map_err(|error| rpc_failure(map_rpc_error("eth_getBalance", error)))?;
        let transaction_value = parse_uint(&transaction.value)?;
        let gas_price = parse_uint(&context.gas_price)?;
        if actual && native_balance < transaction_value {
            anyhow::bail!(SimulationFailure::Reverted(
                "INSUFFICIENT_NATIVE_BALANCE_FOR_GAS"
            ));
        }
        if input.sell_token != NATIVE {
            let balance = self
                .read_balance(&rpc, input.sell_token, taker, block)
                .await
                .map_err(rpc_failure)?;
            if balance < parse_uint(&input.sell_amount)? {
                if actual {
                    anyhow::bail!(SimulationFailure::Reverted("INSUFFICIENT_SELL_BALANCE"));
                }
                let Some(slot) = chain.balance_slots.get(&format!("{:#x}", input.sell_token))
                else {
                    anyhow::bail!(SimulationFailure::Unsupported(
                        "BALANCE_OVERRIDE_SLOT_UNCONFIGURED"
                    ));
                };
                let amount = parse_uint(&input.sell_amount)?;
                let key = balance_storage_key(taker, *slot);
                overrides
                    .entry(input.sell_token)
                    .or_default()
                    .set_state_diff([(key, B256::from(amount.to_be_bytes::<32>()))]);
                let overridden = rpc
                    .call(balance_call(input.sell_token, taker)?)
                    .block(block)
                    .overrides(overrides.clone())
                    .await
                    .map_err(|error| rpc_failure(map_rpc_error("eth_call", error)))?;
                if parse_return_word(&overridden)? != amount {
                    anyhow::bail!(SimulationFailure::Unsupported(
                        "BALANCE_OVERRIDE_VALIDATION_FAILED"
                    ));
                }
            }
            let spender = chain
                .router
                .map_or(route.spender, |_| crate::domain::HOLDER);
            let allowance = rpc
                .call(contract_call(
                    input.sell_token,
                    allowance_data(taker, spender),
                )?)
                .block(block)
                .await
                .map_err(|error| rpc_failure(map_rpc_error("eth_call", error)))?;
            let allowance = parse_return_word(&allowance)?;
            let sell_amount = parse_uint(&input.sell_amount)?;
            if allowance < sell_amount {
                if !allowance.is_zero() {
                    approvals.push(approval(input.sell_token, spender, U256::ZERO));
                }
                approvals.push(approval(input.sell_token, spender, sell_amount));
            }
        }
        let balance_call = balance_call(input.buy_token, taker)?
            .from(taker)
            .gas_limit(100_000)
            .gas_price(0);
        let mut calls = vec![balance_call.clone()];
        calls.extend(
            approvals
                .iter()
                .map(|approval| rpc_transaction(approval, taker, &context.gas_price))
                .collect::<Result<Vec<_>, _>>()?,
        );
        calls.push(rpc_transaction(&transaction, taker, &context.gas_price)?);
        calls.push(balance_call);
        // Nodes check upfront gas-limit cost, not the eventual gas used by a swap.
        // Sum the actual sequential payload, including reset/approval transactions.
        let gas_budget = calls.iter().fold(U256::ZERO, |total, call| {
            total
                + U256::from(call.gas.unwrap_or_default())
                    * U256::from(call.gas_price.unwrap_or_default())
        });
        let required_native = transaction_value + gas_budget;
        if actual && native_balance < required_native {
            anyhow::bail!(SimulationFailure::Reverted(
                "INSUFFICIENT_NATIVE_BALANCE_FOR_GAS"
            ));
        }
        if !actual {
            overrides
                .entry(taker)
                .or_default()
                .set_balance(required_native.max(native_balance));
        }
        let payload = SimulatePayload {
            block_state_calls: vec![SimBlock {
                block_overrides: Some(BlockOverrides {
                    time: Some(context.timestamp.saturating_add(1)),
                    ..Default::default()
                }),
                state_overrides: Some(overrides),
                calls,
            }],
            validation: false,
            trace_transfers: false,
            return_full_transactions: false,
        };
        let raw = rpc
            .simulate(&payload)
            .block_id(block)
            .await
            .map_err(|error| rpc_failure(map_rpc_error("eth_simulateV1", error)))?;
        let call_results = parse_simulation_calls(raw)?;
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
        if bought < parse_uint(min.unwrap_or(&route.min_buy_amount))? {
            anyhow::bail!(SimulationFailure::Reverted(
                "SIMULATED_OUTPUT_BELOW_MINIMUM"
            ));
        }
        let gas_used = call_results[1..call_results.len() - 1]
            .iter()
            .fold(U256::ZERO, |sum, call| sum + U256::from(call.gas_used));
        if actual
            && native_balance
                < transaction_value + gas_used * gas_price * U256::from(12) / U256::from(10)
        {
            anyhow::bail!(SimulationFailure::Reverted(
                "INSUFFICIENT_NATIVE_BALANCE_FOR_GAS"
            ));
        }
        assert_block(&rpc, context).await.map_err(rpc_failure)?;
        let gas_fee = if chain.id == 1 {
            Some((gas_used * gas_price).to_string())
        } else {
            None
        };
        Ok(SimResult {
            simulation: Simulation::Success {
                bought_amount: bought.to_string(),
                gas_used: gas_used.to_string(),
                gas_fee_wei: gas_fee,
                funding: if actual {
                    "actual".into()
                } else {
                    "overridden".into()
                },
                block_hash: context.block_hash.clone(),
            },
            approvals,
            transaction,
        })
    }

    async fn read_balance(
        &self,
        rpc: &DynProvider,
        token: Address,
        owner: Address,
        block: alloy_rpc_types_eth::BlockId,
    ) -> anyhow::Result<U256> {
        let value = rpc
            .call(balance_call(token, owner)?)
            .block(block)
            .await
            .map_err(|error| map_rpc_error("eth_call", error))?;
        parse_return_word(&value)
    }
}

#[async_trait]
impl SimulationProvider for Simulator {
    async fn run(&self, request: SimulationRequest<'_>) -> anyhow::Result<SimResult> {
        Simulator::run(self, request).await
    }
}

async fn assert_block(rpc: &DynProvider, context: &Context) -> anyhow::Result<()> {
    let block = rpc
        .get_block_by_number(block_number(context)?.into())
        .await
        .map_err(|error| map_rpc_error("eth_getBlockByNumber", error))?
        .context(ErrorKind::RpcInvalidResponse)?;
    if format!("{:#x}", block.header.hash) != context.block_hash {
        anyhow::bail!(ErrorKind::ChainReorgRequote);
    }
    Ok(())
}

/// Typed outcome context, not an error container: anyhow retains the original source.
#[derive(Debug, Clone, Copy, thiserror::Error)]
pub enum SimulationFailure {
    #[error("{0}")]
    Reverted(&'static str),
    #[error("{0}")]
    Unsupported(&'static str),
    #[error("{0}")]
    Error(&'static str),
}

impl From<SimulationFailure> for Simulation {
    fn from(failure: SimulationFailure) -> Self {
        match failure {
            SimulationFailure::Reverted(reason) => Self::Reverted {
                reason: reason.into(),
            },
            SimulationFailure::Unsupported(reason) => Self::Unsupported {
                reason: reason.into(),
            },
            SimulationFailure::Error(reason) => Self::Error {
                reason: reason.into(),
            },
        }
    }
}

fn rpc_failure(error: anyhow::Error) -> anyhow::Error {
    let outcome = match crate::error::kind(&error) {
        ErrorKind::RpcMethodUnsupported => SimulationFailure::Unsupported("RPC_METHOD_UNSUPPORTED"),
        ErrorKind::ChainReorgRequote => SimulationFailure::Error("CHAIN_REORG_REQUOTE"),
        ErrorKind::RpcInvalidResponse => SimulationFailure::Error("RPC_INVALID_RESPONSE"),
        _ => SimulationFailure::Error("RPC_CALL_FAILED"),
    };
    error.context(outcome)
}

fn bytes_from_hex(value: &str) -> anyhow::Result<Bytes> {
    let value = parse_hex(value)?;
    Ok(Bytes::from(
        hex::decode(&value[2..]).context(ErrorKind::InvalidCalldata)?,
    ))
}

fn contract_call(to: Address, data: String) -> anyhow::Result<TransactionRequest> {
    Ok(TransactionRequest::default()
        .to(to)
        .input(TransactionInput::both(bytes_from_hex(&data)?)))
}

fn balance_call(token: Address, owner: Address) -> anyhow::Result<TransactionRequest> {
    contract_call(token, balance_data(owner))
}

fn rpc_transaction(
    transaction: &Tx,
    from: Address,
    gas_price: &str,
) -> anyhow::Result<TransactionRequest> {
    Ok(TransactionRequest::default()
        .from(from)
        .to(transaction.to)
        .input(TransactionInput::both(bytes_from_hex(&transaction.data)?))
        .value(parse_uint(&transaction.value)?)
        .gas_limit(0x7a1200)
        .gas_price(parse_uint(gas_price)?.to::<u128>()))
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

fn balance_storage_key(owner: Address, slot: u64) -> B256 {
    let mut encoded = Vec::with_capacity(64);
    encoded.extend_from_slice(&[0_u8; 12]);
    encoded.extend_from_slice(owner.as_slice());
    encoded.extend_from_slice(&U256::from(slot).to_be_bytes::<32>());
    keccak256(encoded)
}

fn parse_simulation_calls(blocks: Vec<SimulatedBlock>) -> anyhow::Result<Vec<SimCallResult>> {
    if blocks.len() != 1 {
        anyhow::bail!(ErrorKind::RpcInvalidResponse);
    }
    Ok(blocks
        .into_iter()
        .next()
        .expect("block count was checked")
        .calls)
}
