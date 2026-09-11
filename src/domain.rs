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
struct InputRequest {
    #[serde(rename = "chainId")]
    chain_id: u64,
    #[serde(rename = "sellToken")]
    sell_token: String,
    #[serde(rename = "buyToken")]
    buy_token: String,
    #[serde(rename = "sellAmount")]
    sell_amount: String,
    #[serde(rename = "slippageBps", default = "default_slippage")]
    slippage_bps: u64,
    taker: Option<String>,
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
    let request: InputRequest =
        serde_json::from_value(value).map_err(|_| Fault::new("INVALID_INPUT"))?;
    if request.chain_id != 1 {
        return Err(Fault::new("INVALID_INPUT"));
    }
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

#[derive(Debug, Clone, Serialize)]
pub struct Token {
    pub address: Address,
    pub symbol: String,
    pub decimals: u8,
    #[serde(skip_serializing)]
    pub price_id: String,
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
    pub rules: std::collections::HashMap<ProviderId, Vec<Rule>>,
    pub balance_slots: std::collections::HashMap<String, u64>,
    pub tokens: Vec<Token>,
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
    #[serde(rename = "nativeUsd")]
    pub native_usd: Option<String>,
    #[serde(rename = "buyUsd")]
    pub buy_usd: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tx {
    pub to: Address,
    pub data: String,
    pub value: String,
}

#[derive(Debug, Clone)]
pub struct Route {
    pub provider: ProviderId,
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
    pub provider: ProviderId,
    pub status: String,
    #[serde(rename = "quotedAmount", skip_serializing_if = "Option::is_none")]
    pub quoted_amount: Option<String>,
    #[serde(rename = "minBuyAmount", skip_serializing_if = "Option::is_none")]
    pub min_buy_amount: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub simulation: Option<Simulation>,
    #[serde(rename = "netOutput", skip_serializing_if = "Option::is_none")]
    pub net_output: Option<Option<String>>,
    #[serde(rename = "latencyMs")]
    pub latency_ms: u64,
    #[serde(rename = "expiresAt")]
    pub expires_at: u64,
    pub execution: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum ProviderId {
    #[serde(rename = "0x")]
    ZeroEx,
    #[serde(rename = "1inch")]
    OneInch,
    #[serde(rename = "kyber")]
    Kyber,
}

impl ProviderId {
    pub const ALL: [Self; 3] = [Self::ZeroEx, Self::OneInch, Self::Kyber];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::ZeroEx => "0x",
            Self::OneInch => "1inch",
            Self::Kyber => "kyber",
        }
    }
}

impl Ord for ProviderId {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.as_str().cmp(other.as_str())
    }
}

impl PartialOrd for ProviderId {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

pub fn minimum(amount: &str, bps: u64) -> Result<String, Fault> {
    let amount = parse_uint(amount)?;
    Ok(format_quantity(
        amount * U256::from(10_000 - bps) / U256::from(10_000),
    ))
}

pub fn price_units(value: &str) -> Result<U256, Fault> {
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    if whole.is_empty()
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(Fault::new("INVALID_PRICE"));
    }
    let fraction = format!("{fraction:0<8}");
    let fraction = &fraction[..fraction.len().min(8)];
    Ok(
        U256::from_str_radix(whole, 10).map_err(|_| Fault::new("INVALID_PRICE"))?
            * U256::from(100_000_000_u64)
            + U256::from_str_radix(fraction, 10).map_err(|_| Fault::new("INVALID_PRICE"))?,
    )
}

pub fn net_output(
    amount: &str,
    fee: Option<&str>,
    context: &Context,
    decimals: u8,
) -> Result<Option<String>, Fault> {
    let (Some(fee), Some(native_usd), Some(buy_usd)) = (
        fee,
        context.native_usd.as_deref(),
        context.buy_usd.as_deref(),
    ) else {
        return Ok(None);
    };
    let amount = parse_uint(amount)?;
    let fee = parse_uint(fee)?;
    let numerator = fee * price_units(native_usd)? * U256::from(10_u64).pow(U256::from(decimals));
    let denominator = U256::from(10_u64).pow(U256::from(18)) * price_units(buy_usd)?;
    if denominator.is_zero() {
        return Ok(None);
    }
    let cost = (numerator + denominator - U256::from(1)) / denominator;
    Ok(Some(format_quantity(amount.saturating_sub(cost))))
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
            && left.net_output.as_ref().is_some_and(Option::is_some);
        let right_verified = right.expires_at > now
            && right
                .simulation
                .as_ref()
                .is_some_and(Simulation::is_success)
            && right.net_output.as_ref().is_some_and(Option::is_some);
        match right_verified.cmp(&left_verified) {
            std::cmp::Ordering::Equal if left_verified => {
                let left_value = parse_uint(
                    left.net_output
                        .as_ref()
                        .and_then(Option::as_ref)
                        .expect("verified quote has net output"),
                );
                let right_value = parse_uint(
                    right
                        .net_output
                        .as_ref()
                        .and_then(Option::as_ref)
                        .expect("verified quote has net output"),
                );
                match (right_value, left_value) {
                    (Ok(right_value), Ok(left_value)) => right_value
                        .cmp(&left_value)
                        .then(left.provider.cmp(&right.provider)),
                    _ => left.provider.cmp(&right.provider),
                }
            }
            std::cmp::Ordering::Equal => left.provider.cmp(&right.provider),
            order => order,
        }
    });
    ranked
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> Context {
        Context {
            block_number: "0x10".into(),
            block_hash: format!("0x{}", "11".repeat(32)),
            timestamp: 100,
            gas_price: "1000000000".into(),
            native_usd: Some("3000".into()),
            buy_usd: Some("1".into()),
        }
    }

    #[test]
    fn integer_math_never_uses_floating_point() {
        assert_eq!(
            minimum("900719925474099312345", 30).unwrap(),
            "898017765697677014407"
        );
        assert_eq!(
            net_output("1000000000", Some("1000000000000000"), &context(), 6).unwrap(),
            Some("997000000".into())
        );
        assert_eq!(
            net_output("100", Some("1"), &context(), 6).unwrap(),
            Some("99".into())
        );
        assert_eq!(net_output("100", None, &context(), 6).unwrap(), None);
        for invalid in ["0", "1.1", "1e18", "-1", "00"] {
            assert!(
                parse_positive(invalid).is_err(),
                "{invalid} must be rejected"
            );
        }
        assert!(
            parse_uint(
                "115792089237316195423570985008687907853269984665640564039457584007913129639936"
            )
            .is_err()
        );
    }

    #[test]
    fn input_rejects_unknown_fields_and_reserved_takers() {
        let base = serde_json::json!({
            "chainId": 1,
            "sellToken": format!("{NATIVE:#x}"),
            "buyToken": "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48",
            "sellAmount": "1"
        });
        assert!(parse_input(base.clone()).is_ok());
        assert!(parse_input(serde_json::json!({"x": 1})).is_err());
        assert!(
            parse_input(serde_json::json!({
                "chainId": 1,
                "sellToken": format!("{NATIVE:#x}"),
                "buyToken": "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48",
                "sellAmount": "1",
                "taker": "0x0000000000000000000000000000000000000001"
            }))
            .is_err()
        );
    }
}
