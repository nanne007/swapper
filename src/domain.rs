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
    pub taker: Option<Address>,
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
    pub taker: Option<String>,
}

fn default_slippage() -> u64 {
    30
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct BuildRequest {
    pub taker: String,
    pub accepted_min_buy_amount: String,
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
    let taker = request
        .taker
        .map(|value| parse_address(&value))
        .transpose()?;
    if taker.is_some_and(|taker| {
        is_reserved_address(taker)
            || U256::from_be_slice(taker.as_slice()) <= U256::from(0xffff_u64)
    }) {
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

pub fn validate_build_request(request: BuildRequest) -> anyhow::Result<(Address, String)> {
    let taker = parse_address(&request.taker)?;
    if U256::from_be_slice(taker.as_slice()) <= U256::from(0xffff_u64) || is_reserved_address(taker)
    {
        anyhow::bail!(ErrorKind::InvalidTaker);
    }
    let accepted = parse_positive(&request.accepted_min_buy_amount)?.to_string();
    Ok((taker, accepted))
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
    pub balance_slots: std::collections::HashMap<String, u64>,
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
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Simulation {
    Success {
        bought_amount: String,
        gas_used: String,
        gas_fee_wei: Option<String>,
        funding: String,
        block_hash: String,
    },
    Reverted {
        reason: String,
    },
    Unsupported {
        reason: String,
    },
    Error {
        reason: String,
    },
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
#[serde(rename_all = "camelCase")]
pub struct Quote {
    pub id: String,
    pub provider: &'static str,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quoted_amount: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_buy_amount: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub simulation: Option<Simulation>,
    pub latency_ms: u64,
    pub expires_at: u64,
    pub execution: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
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
