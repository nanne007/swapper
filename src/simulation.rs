use crate::{
    domain::{
        Address, Chain, Context, Fault, Input, NATIVE, Route, Rule, Simulation, Tx,
        format_quantity, parse_hex, parse_uint,
    },
    execution::{allowance_data, approval, balance_data, swap_transaction},
    rpc::{EvmRpc, RpcFactory, block_id, block_number},
};
use alloy_primitives::{B256, Bytes, U256, keccak256};
use alloy_rpc_types_eth::{
    BlockOverrides, TransactionInput, TransactionRequest,
    simulate::{SimBlock, SimCallResult, SimulatePayload, SimulatedBlock},
    state::StateOverride,
};
use async_trait::async_trait;
use std::{sync::Arc, time::Duration};

pub struct SimResult {
    pub simulation: Simulation,
    pub approvals: Vec<Tx>,
    pub transaction: Tx,
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
    async fn run(&self, request: SimulationRequest<'_>) -> Result<SimResult, Fault>;
}

pub struct Simulator {
    timeout: Duration,
    rpc_factory: Arc<dyn RpcFactory>,
}

impl Simulator {
    pub fn new(timeout: Duration) -> Self {
        Self::with_factory(timeout, Arc::new(crate::rpc::AlloyRpcFactory))
    }

    pub fn with_factory(timeout: Duration, rpc_factory: Arc<dyn RpcFactory>) -> Self {
        Self {
            timeout,
            rpc_factory,
        }
    }

    pub async fn run(&self, request: SimulationRequest<'_>) -> Result<SimResult, Fault> {
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
            return Ok(failure(
                SimulationStatus::Unsupported,
                "RPC_NOT_CONFIGURED",
                &approvals,
                &transaction,
            ));
        };
        let rpc = self.rpc_factory.connect(url, self.timeout)?;
        if let Err(error) = assert_block(rpc.as_ref(), context).await {
            return Ok(SimResult {
                simulation: classify_fault(error),
                approvals,
                transaction,
            });
        }
        let block = block_id(context)?;
        if let Some(router) = chain.router {
            let code = match rpc.code_at(router, block).await {
                Ok(value) => value,
                Err(error) => return Ok(rpc_failure(error, &approvals, &transaction)),
            };
            if code.is_empty() {
                return Ok(failure(
                    SimulationStatus::Unsupported,
                    "ROUTER_NOT_DEPLOYED",
                    &approvals,
                    &transaction,
                ));
            }
        }
        let mut overrides = StateOverride::default();
        let native_balance = match rpc.balance_at(taker, block).await {
            Ok(value) => value,
            Err(error) => {
                return Ok(failure(
                    SimulationStatus::Error,
                    &error.code,
                    &approvals,
                    &transaction,
                ));
            }
        };
        let transaction_value = parse_uint(&transaction.value)?;
        let gas_price = parse_uint(&context.gas_price)?;
        let required_native = transaction_value + gas_price * U256::from(2_000_000_u64);
        if actual && native_balance < required_native {
            return Ok(failure(
                SimulationStatus::Reverted,
                "INSUFFICIENT_NATIVE_BALANCE_FOR_GAS",
                &approvals,
                &transaction,
            ));
        }
        if !actual {
            overrides
                .entry(taker)
                .or_default()
                .set_balance(required_native.max(native_balance));
        }
        if input.sell_token != NATIVE {
            let balance = match self
                .read_balance(rpc.as_ref(), input.sell_token, taker, block)
                .await
            {
                Ok(value) => value,
                Err(error) => return Ok(rpc_failure(error, &approvals, &transaction)),
            };
            if balance < parse_uint(&input.sell_amount)? {
                if actual {
                    return Ok(failure(
                        SimulationStatus::Reverted,
                        "INSUFFICIENT_SELL_BALANCE",
                        &approvals,
                        &transaction,
                    ));
                }
                let Some(slot) = chain.balance_slots.get(&format!("{:#x}", input.sell_token))
                else {
                    return Ok(failure(
                        SimulationStatus::Unsupported,
                        "BALANCE_OVERRIDE_SLOT_UNCONFIGURED",
                        &approvals,
                        &transaction,
                    ));
                };
                let amount = parse_uint(&input.sell_amount)?;
                let key = balance_storage_key(taker, *slot);
                overrides
                    .entry(input.sell_token)
                    .or_default()
                    .set_state_diff([(key, B256::from(amount.to_be_bytes::<32>()))]);
                let overridden = match rpc
                    .call(
                        balance_call(input.sell_token, taker)?,
                        block,
                        Some(overrides.clone()),
                    )
                    .await
                {
                    Ok(value) => value,
                    Err(error) => return Ok(rpc_failure(error, &approvals, &transaction)),
                };
                if parse_return_word(&overridden)? != amount {
                    return Ok(failure(
                        SimulationStatus::Unsupported,
                        "BALANCE_OVERRIDE_VALIDATION_FAILED",
                        &approvals,
                        &transaction,
                    ));
                }
            }
            let spender = chain
                .router
                .map_or(route.spender, |_| crate::domain::HOLDER);
            let allowance = match rpc
                .call(
                    contract_call(input.sell_token, allowance_data(taker, spender))?,
                    block,
                    None,
                )
                .await
            {
                Ok(value) => value,
                Err(error) => return Ok(rpc_failure(error, &approvals, &transaction)),
            };
            let allowance = parse_return_word(&allowance)?;
            let sell_amount = parse_uint(&input.sell_amount)?;
            if allowance < sell_amount {
                if !allowance.is_zero() {
                    approvals.push(approval(input.sell_token, spender, U256::ZERO));
                }
                approvals.push(approval(input.sell_token, spender, sell_amount));
            }
        }
        let balance_call = balance_call(input.buy_token, taker)?.gas_limit(100_000);
        let mut calls = vec![balance_call.clone()];
        calls.extend(
            approvals
                .iter()
                .map(|approval| rpc_transaction(approval, taker, &context.gas_price))
                .collect::<Result<Vec<_>, _>>()?,
        );
        calls.push(rpc_transaction(&transaction, taker, &context.gas_price)?);
        calls.push(balance_call);
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
        let raw = match rpc.simulate(payload, block).await {
            Ok(value) => value,
            Err(error) => return Ok(rpc_failure(error, &approvals, &transaction)),
        };
        let call_results = parse_simulation_calls(raw)?;
        if call_results.len() != approvals.len() + 3 {
            return Ok(failure(
                SimulationStatus::Error,
                "SIMULATION_CALL_COUNT_MISMATCH",
                &approvals,
                &transaction,
            ));
        }
        if call_results.iter().any(|call| !call.status) {
            return Ok(failure(
                SimulationStatus::Reverted,
                "SIMULATION_REVERTED",
                &approvals,
                &transaction,
            ));
        }
        if call_results[1..1 + approvals.len()]
            .iter()
            .any(|call| !call.return_data.is_empty() && call.return_data != word_bytes(U256::ONE))
        {
            return Ok(failure(
                SimulationStatus::Reverted,
                "APPROVAL_RETURNED_FALSE",
                &approvals,
                &transaction,
            ));
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
            return Ok(failure(
                SimulationStatus::Reverted,
                "SIMULATION_INVALID_BALANCE_DELTA",
                &approvals,
                &transaction,
            ));
        }
        let bought = after - before;
        if bought < parse_uint(min.unwrap_or(&route.min_buy_amount))? {
            return Ok(failure(
                SimulationStatus::Reverted,
                "SIMULATED_OUTPUT_BELOW_MINIMUM",
                &approvals,
                &transaction,
            ));
        }
        let gas_used = call_results[1..call_results.len() - 1]
            .iter()
            .fold(U256::ZERO, |sum, call| sum + U256::from(call.gas_used));
        if actual
            && native_balance
                < transaction_value + gas_used * gas_price * U256::from(12) / U256::from(10)
        {
            return Ok(failure(
                SimulationStatus::Reverted,
                "INSUFFICIENT_NATIVE_BALANCE_FOR_GAS",
                &approvals,
                &transaction,
            ));
        }
        if let Err(error) = assert_block(rpc.as_ref(), context).await {
            return Ok(SimResult {
                simulation: classify_fault(error),
                approvals,
                transaction,
            });
        }
        let gas_fee = if chain.id == 1 {
            Some(format_quantity(gas_used * gas_price))
        } else {
            None
        };
        Ok(SimResult {
            simulation: Simulation::Success {
                bought_amount: format_quantity(bought),
                gas_used: format_quantity(gas_used),
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
        rpc: &dyn EvmRpc,
        token: Address,
        owner: Address,
        block: alloy_rpc_types_eth::BlockId,
    ) -> Result<U256, Fault> {
        let value = rpc.call(balance_call(token, owner)?, block, None).await?;
        parse_return_word(&value)
    }
}

#[async_trait]
impl SimulationProvider for Simulator {
    async fn run(&self, request: SimulationRequest<'_>) -> Result<SimResult, Fault> {
        Simulator::run(self, request).await
    }
}

async fn assert_block(rpc: &dyn EvmRpc, context: &Context) -> Result<(), Fault> {
    let block = rpc.block_by_number(block_number(context)?).await?;
    if format!("{:#x}", block.hash) != context.block_hash {
        return Err(Fault::with_status("CHAIN_REORG_REQUOTE", 409));
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum SimulationStatus {
    Reverted,
    Unsupported,
    Error,
}

fn status(status: SimulationStatus, reason: &str) -> Simulation {
    match status {
        SimulationStatus::Reverted => Simulation::Reverted {
            reason: reason.into(),
        },
        SimulationStatus::Unsupported => Simulation::Unsupported {
            reason: reason.into(),
        },
        SimulationStatus::Error => Simulation::Error {
            reason: reason.into(),
        },
    }
}

fn failure(kind: SimulationStatus, reason: &str, approvals: &[Tx], transaction: &Tx) -> SimResult {
    SimResult {
        simulation: status(kind, reason),
        approvals: approvals.to_vec(),
        transaction: transaction.clone(),
    }
}

fn classify_fault(error: Fault) -> Simulation {
    match error.code.as_str() {
        "RPC_METHOD_UNSUPPORTED" => Simulation::Unsupported { reason: error.code },
        "CHAIN_REORG_REQUOTE" => Simulation::Error { reason: error.code },
        _ => Simulation::Error { reason: error.code },
    }
}

fn rpc_failure(error: Fault, approvals: &[Tx], transaction: &Tx) -> SimResult {
    let kind = if error.code == "RPC_METHOD_UNSUPPORTED" {
        SimulationStatus::Unsupported
    } else {
        SimulationStatus::Error
    };
    failure(kind, &error.code, approvals, transaction)
}

fn bytes_from_hex(value: &str) -> Result<Bytes, Fault> {
    let value = parse_hex(value)?;
    Ok(Bytes::from(hex::decode(&value[2..]).map_err(|_| {
        Fault::with_status("INVALID_CALLDATA", 502)
    })?))
}

fn contract_call(to: Address, data: String) -> Result<TransactionRequest, Fault> {
    Ok(TransactionRequest::default()
        .to(to)
        .input(TransactionInput::both(bytes_from_hex(&data)?)))
}

fn balance_call(token: Address, owner: Address) -> Result<TransactionRequest, Fault> {
    contract_call(token, balance_data(owner))
}

fn rpc_transaction(
    transaction: &Tx,
    from: Address,
    gas_price: &str,
) -> Result<TransactionRequest, Fault> {
    Ok(TransactionRequest::default()
        .from(from)
        .to(transaction.to)
        .input(TransactionInput::both(bytes_from_hex(&transaction.data)?))
        .value(parse_uint(&transaction.value)?)
        .gas_limit(0x7a1200)
        .gas_price(parse_uint(gas_price)?.to::<u128>()))
}

fn parse_return_word(value: &Bytes) -> Result<U256, Fault> {
    if value.len() != 32 {
        return Err(Fault::with_status("RPC_INVALID_RESPONSE", 502));
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

fn parse_simulation_calls(blocks: Vec<SimulatedBlock>) -> Result<Vec<SimCallResult>, Fault> {
    if blocks.len() != 1 {
        return Err(Fault::with_status("RPC_INVALID_RESPONSE", 502));
    }
    Ok(blocks
        .into_iter()
        .next()
        .expect("block count was checked")
        .calls)
}
