use crate::error::ErrorKind;
pub use alloy_primitives::Address;
use alloy_primitives::U256;
use anyhow::Context as _;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

pub const NATIVE: Address = Address::new([0xee; 20]);
pub const HOLDER: Address = Address::new([
    0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0xff, 0x36, 0x84, 0xf2, 0x8c, 0x67, 0x53, 0x8d, 0x4d, 0x07,
    0x2c, 0x22, 0x73, 0x4,
]);
// Low 20 bytes of keccak256("MetaMatch preview account"). Velora rejects 0x...0a11ce.
// No signing key is held for this reserved account; it is used only with funding overrides.
pub const PREVIEW_TAKER: Address =
    alloy_primitives::address!("b6d846be89cacda845610ca2b26d0635f1db90af");

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Input {
    pub chain_id: u64,
    pub sell_token: Address,
    pub buy_token: Address,
    pub sell_amount: String,
    pub slippage_bps: u64,
    pub taker: Address,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct CreateCompetitionRequest {
    pub chain_id: u64,
    pub sell_token: String,
    pub buy_token: String,
    pub sell_amount: String,
    #[serde(default = "default_slippage")]
    pub slippage_bps: u64,
    pub taker: String,
}

fn default_slippage() -> u64 {
    30
}

pub fn validate_input(request: CreateCompetitionRequest) -> anyhow::Result<Input> {
    let sell_token = parse_address(&request.sell_token)?;
    let buy_token = parse_address(&request.buy_token)?;
    if sell_token == buy_token || buy_token == NATIVE {
        anyhow::bail!(ErrorKind::InvalidInput);
    }
    if !(1..=500).contains(&request.slippage_bps) {
        anyhow::bail!(ErrorKind::InvalidInput);
    }
    let sell_amount = parse_positive(&request.sell_amount)?.to_string();
    let taker = parse_address(&request.taker)?;
    if is_reserved_address(taker) || U256::from_be_slice(taker.as_slice()) <= U256::from(0xffff_u64)
    {
        anyhow::bail!(ErrorKind::InvalidTaker);
    }
    Ok(Input {
        chain_id: request.chain_id,
        sell_token,
        buy_token,
        sell_amount,
        slippage_bps: request.slippage_bps,
        taker,
    })
}

pub fn parse_address(value: &str) -> anyhow::Result<Address> {
    if value.len() != 42 || !value.starts_with("0x") {
        anyhow::bail!(ErrorKind::InvalidInput);
    }
    Address::from_str(value).context(ErrorKind::InvalidInput)
}

pub fn parse_hex(value: &str) -> anyhow::Result<String> {
    if !value.starts_with("0x") || value.len() < 4 || !value.len().is_multiple_of(2) {
        anyhow::bail!(ErrorKind::InvalidInput);
    }
    hex::decode(&value[2..]).context(ErrorKind::InvalidInput)?;
    Ok(value.to_ascii_lowercase())
}

pub fn parse_hex_quantity(value: &str) -> anyhow::Result<U256> {
    if !value.starts_with("0x") || value.len() < 3 {
        anyhow::bail!(ErrorKind::RpcInvalidResponse);
    }
    U256::from_str_radix(&value[2..], 16).context(ErrorKind::RpcInvalidResponse)
}

pub fn parse_uint(value: &str) -> anyhow::Result<U256> {
    if value.is_empty()
        || (value.len() > 1 && value.starts_with('0'))
        || value.len() > 78
        || !value.bytes().all(|byte| byte.is_ascii_digit())
    {
        anyhow::bail!(ErrorKind::InvalidInput);
    }
    U256::from_str_radix(value, 10).context(ErrorKind::InvalidInput)
}

pub fn parse_positive(value: &str) -> anyhow::Result<U256> {
    let parsed = parse_uint(value)?;
    if parsed.is_zero() {
        anyhow::bail!(ErrorKind::InvalidInput);
    }
    Ok(parsed)
}

pub fn is_reserved_address(value: Address) -> bool {
    value == HOLDER || value == NATIVE || value == Address::ZERO
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    pub target: Address,
    pub spender: Address,
    pub selector: String,
}

#[derive(Debug, Clone)]
pub struct Chain {
    pub id: u64,
    pub name: String,
    pub rpc_url: Option<String>,
    pub router: Option<Address>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Context {
    pub block_number: String,
    pub block_hash: String,
    pub timestamp: u64,
    pub gas_price: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tx {
    pub to: Address,
    pub data: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Route {
    pub provider: &'static str,
    pub buy_amount: String,
    pub min_buy_amount: String,
    pub sell_amount: String,
    pub spender: Address,
    pub tx: Tx,
    /// Upstream execution deadline in Unix seconds, when supplied by the provider.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deadline: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SimulationSuccess {
    pub bought_amount: String,
    pub gas_used: String,
    pub gas_fee_wei: Option<String>,
    pub funding: String,
    pub block_context: BlockContext,
    pub simulated_timestamp: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BlockContext {
    pub number: String,
    pub hash: String,
    pub timestamp: u64,
}

impl Context {
    pub fn block_context(&self) -> BlockContext {
        BlockContext {
            number: self.block_number.clone(),
            hash: self.block_hash.clone(),
            timestamp: self.timestamp,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Quote {
    pub route: Route,
    pub simulation: SimulationSuccess,
    pub approvals: Vec<Tx>,
    pub transaction: Tx,
    pub latency_ms: u64,
}

pub fn minimum(amount: &str, bps: u64) -> anyhow::Result<String> {
    let amount = parse_uint(amount)?;
    Ok((amount * U256::from(10_000 - bps) / U256::from(10_000)).to_string())
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock must be after unix epoch")
        .as_millis() as u64
}

pub fn rank(quotes: Vec<Quote>) -> anyhow::Result<Vec<Quote>> {
    let mut scored = quotes
        .into_iter()
        .map(|quote| {
            let amount = parse_positive(&quote.simulation.bought_amount)?;
            Ok((quote, amount))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    scored.sort_by(|(left, left_amount), (right, right_amount)| {
        right_amount
            .cmp(left_amount)
            .then(left.route.provider.cmp(right.route.provider))
    });
    Ok(scored.into_iter().map(|(quote, _)| quote).collect())
}
