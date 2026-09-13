pub use alloy_primitives::Address;
use alloy_primitives::U256;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use thiserror::Error;

pub const NATIVE: Address = Address::new([0xee; 20]);
pub const HOLDER: Address = Address::new([
    0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0xff, 0x36, 0x84, 0xf2, 0x8c, 0x67, 0x53, 0x8d, 0x4d, 0x07,
    0x2c, 0x22, 0x73, 0x4,
]);
pub const PREVIEW_TAKER: Address = Address::new([
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x0a, 0x11, 0xce,
]);

#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("{code}")]
pub struct Fault {
    pub code: String,
    pub http_status: u16,
}

impl Fault {
    pub fn new(code: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            http_status: 400,
        }
    }

    pub fn with_status(code: impl Into<String>, http_status: u16) -> Self {
        Self {
            code: code.into(),
            http_status,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Input {
    #[serde(rename = "chainId")]
    pub chain_id: u64,
    #[serde(rename = "sellToken")]
    pub sell_token: Address,
    #[serde(rename = "buyToken")]
    pub buy_token: Address,
    #[serde(rename = "sellAmount")]
    pub sell_amount: String,
    #[serde(rename = "slippageBps")]
    pub slippage_bps: u64,
    pub taker: Option<Address>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateCompetitionRequest {
    #[serde(rename = "chainId")]
    pub chain_id: u64,
    #[serde(rename = "sellToken")]
    pub sell_token: String,
    #[serde(rename = "buyToken")]
    pub buy_token: String,
    #[serde(rename = "sellAmount")]
    pub sell_amount: String,
    #[serde(rename = "slippageBps", default = "default_slippage")]
    pub slippage_bps: u64,
    pub taker: Option<String>,
}

fn default_slippage() -> u64 {
    30
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildRequest {
    pub taker: String,
    #[serde(rename = "acceptedMinBuyAmount")]
    pub accepted_min_buy_amount: String,
}

pub fn parse_input(value: serde_json::Value) -> Result<Input, Fault> {
    let request: CreateCompetitionRequest =
        serde_json::from_value(value).map_err(|_| Fault::new("INVALID_INPUT"))?;
    validate_input(request)
}

pub fn validate_input(request: CreateCompetitionRequest) -> Result<Input, Fault> {
    let sell_token = parse_address(&request.sell_token)?;
    let buy_token = parse_address(&request.buy_token)?;
    if sell_token == buy_token || buy_token == NATIVE {
        return Err(Fault::new("INVALID_INPUT"));
    }
    if !(1..=500).contains(&request.slippage_bps) {
        return Err(Fault::new("INVALID_INPUT"));
    }
    let sell_amount = parse_positive(&request.sell_amount)?.to_string();
    let taker = request
        .taker
        .map(|value| parse_address(&value))
        .transpose()?;
    if taker.is_some_and(|taker| {
        is_reserved_address(taker)
            || U256::from_be_slice(taker.as_slice()) <= U256::from(0xffff_u64)
    }) {
        return Err(Fault::new("INVALID_TAKER"));
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

pub fn parse_build_request(value: serde_json::Value) -> Result<(Address, String), Fault> {
    let request: BuildRequest =
        serde_json::from_value(value).map_err(|_| Fault::new("INVALID_INPUT"))?;
    validate_build_request(request)
}

pub fn validate_build_request(request: BuildRequest) -> Result<(Address, String), Fault> {
    let taker = parse_address(&request.taker)?;
    if U256::from_be_slice(taker.as_slice()) <= U256::from(0xffff_u64) || is_reserved_address(taker)
    {
        return Err(Fault::new("INVALID_TAKER"));
    }
    let accepted = parse_positive(&request.accepted_min_buy_amount)?.to_string();
    Ok((taker, accepted))
}

pub fn parse_address(value: &str) -> Result<Address, Fault> {
    if value.len() != 42 || !value.starts_with("0x") {
        return Err(Fault::new("INVALID_INPUT"));
    }
    Address::from_str(value).map_err(|_| Fault::new("INVALID_INPUT"))
}

pub fn parse_hex(value: &str) -> Result<String, Fault> {
    if !value.starts_with("0x") || value.len() < 4 || !value.len().is_multiple_of(2) {
        return Err(Fault::new("INVALID_INPUT"));
    }
    hex::decode(&value[2..]).map_err(|_| Fault::new("INVALID_INPUT"))?;
    Ok(value.to_ascii_lowercase())
}

pub fn parse_hex_quantity(value: &str) -> Result<U256, Fault> {
    if !value.starts_with("0x") || value.len() < 3 {
        return Err(Fault::new("RPC_INVALID_RESPONSE"));
    }
    U256::from_str_radix(&value[2..], 16).map_err(|_| Fault::new("RPC_INVALID_RESPONSE"))
}

pub fn parse_uint(value: &str) -> Result<U256, Fault> {
    if value.is_empty()
        || (value.len() > 1 && value.starts_with('0'))
        || value.len() > 78
        || !value.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(Fault::new("INVALID_INPUT"));
    }
    U256::from_str_radix(value, 10).map_err(|_| Fault::new("INVALID_INPUT"))
}

pub fn parse_positive(value: &str) -> Result<U256, Fault> {
    let parsed = parse_uint(value)?;
    if parsed.is_zero() {
        return Err(Fault::new("INVALID_INPUT"));
    }
    Ok(parsed)
}

pub fn format_quantity(value: U256) -> String {
    value.to_string()
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
    pub slug: String,
    pub rpc_url: Option<String>,
    pub router: Option<Address>,
    pub balance_slots: std::collections::HashMap<String, u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Context {
    #[serde(rename = "blockNumber")]
    pub block_number: String,
    #[serde(rename = "blockHash")]
    pub block_hash: String,
    pub timestamp: u64,
    #[serde(rename = "gasPrice")]
    pub gas_price: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tx {
    pub to: Address,
    pub data: String,
    pub value: String,
}

#[derive(Debug, Clone)]
pub struct Route {
    pub provider: &'static str,
    pub buy_amount: String,
    pub min_buy_amount: String,
    pub sell_amount: String,
    pub spender: Address,
    pub tx: Tx,
    pub expires_at: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status")]
pub enum Simulation {
    #[serde(rename = "success")]
    Success {
        #[serde(rename = "boughtAmount")]
        bought_amount: String,
        #[serde(rename = "gasUsed")]
        gas_used: String,
        #[serde(rename = "gasFeeWei")]
        gas_fee_wei: Option<String>,
        funding: String,
        #[serde(rename = "blockHash")]
        block_hash: String,
    },
    #[serde(rename = "reverted")]
    Reverted { reason: String },
    #[serde(rename = "unsupported")]
    Unsupported { reason: String },
    #[serde(rename = "error")]
    Error { reason: String },
}

impl Simulation {
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Success { .. })
    }

    pub fn is_actual_success(&self) -> bool {
        matches!(self, Self::Success { funding, .. } if funding == "actual")
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Quote {
    pub id: String,
    pub provider: &'static str,
    pub status: String,
    #[serde(rename = "quotedAmount", skip_serializing_if = "Option::is_none")]
    pub quoted_amount: Option<String>,
    #[serde(rename = "minBuyAmount", skip_serializing_if = "Option::is_none")]
    pub min_buy_amount: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub simulation: Option<Simulation>,
    #[serde(rename = "latencyMs")]
    pub latency_ms: u64,
    #[serde(rename = "expiresAt")]
    pub expires_at: u64,
    pub execution: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub fn minimum(amount: &str, bps: u64) -> Result<String, Fault> {
    let amount = parse_uint(amount)?;
    Ok(format_quantity(
        amount * U256::from(10_000 - bps) / U256::from(10_000),
    ))
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock must be after unix epoch")
        .as_millis() as u64
}

pub fn rank(quotes: &[Quote]) -> Vec<Quote> {
    let now = now_ms();
    let mut ranked = quotes.to_vec();
    ranked.sort_by(|left, right| {
        let left_verified = left.expires_at > now
            && left.simulation.as_ref().is_some_and(Simulation::is_success)
            && left.quoted_amount.is_some();
        let right_verified = right.expires_at > now
            && right
                .simulation
                .as_ref()
                .is_some_and(Simulation::is_success)
            && right.quoted_amount.is_some();
        match right_verified.cmp(&left_verified) {
            std::cmp::Ordering::Equal if left_verified => {
                let left_value = parse_uint(
                    left.quoted_amount
                        .as_ref()
                        .expect("verified quote has quoted amount"),
                );
                let right_value = parse_uint(
                    right
                        .quoted_amount
                        .as_ref()
                        .expect("verified quote has quoted amount"),
                );
                match (right_value, left_value) {
                    (Ok(right_value), Ok(left_value)) => right_value
                        .cmp(&left_value)
                        .then(left.provider.cmp(right.provider)),
                    _ => left.provider.cmp(right.provider),
                }
            }
            std::cmp::Ordering::Equal => left.provider.cmp(right.provider),
            order => order,
        }
    });
    ranked
}
