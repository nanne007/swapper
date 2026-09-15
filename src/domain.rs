use crate::error::ErrorKind;
pub use alloy_primitives::Address;
use alloy_primitives::{Bytes, U256};
use anyhow::Context as _;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

pub const NATIVE: Address = Address::new([0xee; 20]);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Input {
    #[serde(deserialize_with = "deserialize_chain_id")]
    pub chain_id: u64,
    #[serde(deserialize_with = "deserialize_address")]
    pub sell_token: Address,
    #[serde(deserialize_with = "deserialize_address")]
    pub buy_token: Address,
    #[serde(deserialize_with = "deserialize_positive_amount")]
    pub sell_amount: String,
    #[serde(
        default = "default_slippage",
        deserialize_with = "deserialize_slippage"
    )]
    pub slippage_bps: u64,
    #[serde(deserialize_with = "deserialize_address")]
    pub taker: Address,
}

fn deserialize_chain_id<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<u64, D::Error> {
    std::num::NonZeroU64::deserialize(deserializer).map(std::num::NonZeroU64::get)
}

fn deserialize_address<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Address, D::Error> {
    // Alloy also accepts byte arrays and unprefixed hex; the API requires address strings.
    let value = String::deserialize(deserializer)?;
    parse_address(&value).map_err(serde::de::Error::custom)
}

fn deserialize_positive_amount<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<String, D::Error> {
    let value = String::deserialize(deserializer)?;
    parse_positive(&value).map_err(serde::de::Error::custom)?;
    Ok(value)
}

fn deserialize_slippage<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<u64, D::Error> {
    let value = u64::deserialize(deserializer)?;
    if !(1..=500).contains(&value) {
        return Err(serde::de::Error::custom(
            "slippageBps must be between 1 and 500",
        ));
    }
    Ok(value)
}

fn default_slippage() -> u64 {
    30
}

impl Input {
    /// Cross-field checks stay explicit so reserved takers retain their API error kind.
    /// Field syntax and ranges are checked by Serde at the JSON boundary.
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.sell_token == self.buy_token
            || self.buy_token == NATIVE
            || self.sell_token == Address::ZERO
            || self.buy_token == Address::ZERO
        {
            anyhow::bail!(ErrorKind::InvalidInput);
        }
        if is_reserved_address(self.taker)
            || U256::from_be_slice(self.taker.as_slice()) <= U256::from(0xffff_u64)
        {
            anyhow::bail!(ErrorKind::InvalidTaker);
        }
        Ok(())
    }
}

pub fn parse_address(value: &str) -> anyhow::Result<Address> {
    if value.len() != 42 || !value.starts_with("0x") {
        anyhow::bail!(ErrorKind::InvalidInput);
    }
    Address::from_str(value).context(ErrorKind::InvalidInput)
}

pub fn parse_hex(value: &str) -> anyhow::Result<Bytes> {
    if !value.starts_with("0x") || value.len() < 4 || !value.len().is_multiple_of(2) {
        anyhow::bail!(ErrorKind::InvalidInput);
    }
    hex::decode(&value[2..])
        .map(Bytes::from)
        .context(ErrorKind::InvalidInput)
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
    value == NATIVE || value == Address::ZERO
}

#[derive(Debug, Clone)]
pub struct Chain {
    pub id: u64,
    pub name: String,
    pub rpc_url: Option<String>,
    pub router: Option<RouterDeployment>,
}

/// A configured router is executable only together with the Holder read during bootstrap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouterDeployment {
    pub address: Address,
    pub holder: Address,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tx {
    pub to: Address,
    pub data: Bytes,
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
    pub number: u64,
    pub hash: String,
    pub timestamp: u64,
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
    let factor = U256::from(
        10_000_u64
            .checked_sub(bps)
            .context(ErrorKind::InvalidInput)?,
    );
    let scale = U256::from(10_000);
    // Keep floor(amount * factor / scale) without overflowing the intermediate product.
    Ok(((amount / scale) * factor + (amount % scale) * factor / scale).to_string())
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
