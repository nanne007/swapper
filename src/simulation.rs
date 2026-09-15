use crate::error::ErrorKind;
use crate::{
    balance_slots::{BalanceSlotError, BalanceSlots, storage_key},
    domain::{
        Address, Chain, Context, Input, NATIVE, RouterDeployment, SimulationSuccess, Tx, parse_uint,
    },
    execution::{ValidatedRoute, allowance_data, approval, balance_data, swap_transaction},
    rpc::{RpcClients, block_id, block_number, map_rpc_error},
};
use alloy_primitives::{B256, Bytes, U256};
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
    pub context: &'a Context,
    pub preparation: &'a SimulationPreparation,
}

/// Immutable, request-local prerequisites. Each route builds its own state overrides.
pub struct SimulationPreparation {
    pub router: RouterDeployment,
    pub approvals: Vec<Tx>,
    pub sell_balance_slot: Option<B256>,
}

#[async_trait]
pub trait SimulationProvider: Send + Sync {
    async fn prepare(
        &self,
        input: &Input,
        chain: &Chain,
        context: &Context,
    ) -> anyhow::Result<SimulationPreparation>;
    async fn run(&self, request: SimulationRequest<'_>) -> anyhow::Result<SimResult>;
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

    pub async fn prepare(
        &self,
        input: &Input,
        chain: &Chain,
        context: &Context,
    ) -> anyhow::Result<SimulationPreparation> {
        let router = chain.router.context(ErrorKind::RouterNotConfigured)?;
        let rpc = self.rpc(chain)?;
        assert_block(&rpc, context).await.map_err(rpc_failure)?;
        let block = block_id(context)?;
        let code = async {
            let code = rpc
                .get_code_at(router.address)
                .block_id(block)
                .await
                .map_err(|error| rpc_failure(map_rpc_error("eth_getCode", error)))?;
            if code.is_empty() {
                anyhow::bail!(SimulationFailure::Unsupported("ROUTER_NOT_DEPLOYED"));
            }
            Ok::<_, anyhow::Error>(())
        };
        let funding = async {
            if input.sell_token == NATIVE {
                return Ok((None, Vec::new()));
            }
            let base = async {
                self.balance_slots
                    .resolve(&rpc, chain.id, input.sell_token, block)
                    .await
                    .map_err(balance_slot_failure)
            };
            let allowance = async {
                let data = rpc
                    .call(contract_call(
                        input.sell_token,
                        allowance_data(input.taker, router.holder),
                    ))
                    .block(block)
                    .await
                    .map_err(|error| rpc_failure(map_rpc_error("eth_call", error)))?;
                parse_return_word(&data)
            };
            let (base, allowance) = tokio::try_join!(base, allowance)?;
            let sell_amount = parse_uint(&input.sell_amount)?;
            let mut approvals = Vec::new();
            if allowance < sell_amount {
                if !allowance.is_zero() {
                    approvals.push(approval(input.sell_token, router.holder, U256::ZERO));
                }
                approvals.push(approval(input.sell_token, router.holder, sell_amount));
            }
            Ok::<_, anyhow::Error>((Some(storage_key(input.taker, base)), approvals))
        };
        let ((), (sell_balance_slot, approvals)) = tokio::try_join!(code, funding)?;
        Ok(SimulationPreparation {
            router,
            approvals,
            sell_balance_slot,
        })
    }

    fn rpc(&self, chain: &Chain) -> anyhow::Result<DynProvider> {
        let Some(url) = &chain.rpc_url else {
            anyhow::bail!(SimulationFailure::Unsupported("RPC_NOT_CONFIGURED"));
        };
        self.clients.get(url)
    }

    pub async fn run(&self, request: SimulationRequest<'_>) -> anyhow::Result<SimResult> {
        let SimulationRequest {
            input,
            chain,
            route,
            context,
            preparation,
        } = request;
        let transaction = swap_transaction(input, preparation.router, route)?;
        let approvals = &preparation.approvals;
        let taker = input.taker;
        let rpc = self.rpc(chain)?;
        let block = block_id(context)?;
        let mut overrides = match preparation.sell_balance_slot {
            Some(slot) => {
                token_balance_override(input.sell_token, slot, parse_uint(&input.sell_amount)?)
            }
            None => StateOverride::default(),
        };
        let transaction_value = parse_uint(&transaction.value)?;
        let gas_price = parse_uint(&context.gas_price)?;
        let balance_call = contract_call(input.buy_token, balance_data(taker))
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
        overrides
            .entry(taker)
            .or_default()
            .set_balance(required_native);
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
        if bought < parse_uint(&route.min_buy_amount)? {
            anyhow::bail!(SimulationFailure::Reverted(
                "SIMULATED_OUTPUT_BELOW_MINIMUM"
            ));
        }
        let gas_used = call_results[1..call_results.len() - 1]
            .iter()
            .fold(U256::ZERO, |sum, call| sum + U256::from(call.gas_used));
        assert_block(&rpc, context).await.map_err(rpc_failure)?;
        let gas_fee = if chain.id == 1 {
            Some((gas_used * gas_price).to_string())
        } else {
            None
        };
        Ok(SimResult {
            simulation: SimulationSuccess {
                bought_amount: bought.to_string(),
                gas_used: gas_used.to_string(),
                gas_fee_wei: gas_fee,
                funding: "overridden".into(),
                block_context: context.block_context(),
                simulated_timestamp: context.timestamp.saturating_add(1),
            },
            approvals: approvals.clone(),
            transaction,
        })
    }
}

#[async_trait]
impl SimulationProvider for Simulator {
    async fn prepare(
        &self,
        input: &Input,
        chain: &Chain,
        context: &Context,
    ) -> anyhow::Result<SimulationPreparation> {
        Simulator::prepare(self, input, chain, context).await
    }
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
        ErrorKind::ChainReorgRequote => SimulationFailure::Error("CHAIN_REORG_REQUOTE"),
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

fn rpc_transaction(
    transaction: &Tx,
    from: Address,
    gas_price: &str,
) -> anyhow::Result<TransactionRequest> {
    Ok(TransactionRequest::default()
        .from(from)
        .to(transaction.to)
        .input(TransactionInput::new(transaction.data.clone()))
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
