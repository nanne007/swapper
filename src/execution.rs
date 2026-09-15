use crate::domain::{Chain, HOLDER, Input, NATIVE, Route, Rule, Tx, parse_hex, parse_uint};
use crate::error::ErrorKind;
use alloy_primitives::{Address, Bytes, U256};
use alloy_sol_types::{SolCall, sol};
use anyhow::Context as _;

sol! {
    function approve(address spender, uint256 amount) returns (bool);
    function balanceOf(address owner) view returns (uint256);
    function allowance(address owner, address spender) view returns (uint256);
    function execute(address sellToken, address buyToken, uint256 sellAmount, uint256 minBuyAmount, address target, address spender, uint256 value, bytes data, uint256 deadline) payable returns (uint256);
    function exec(address operator, address token, uint256 amount, address target, bytes data) payable returns (bytes);
}

pub fn validate_route(
    input: &Input,
    chain: &Chain,
    route: &Route,
    rules: &[Rule],
    require_unified: bool,
) -> anyhow::Result<()> {
    if route.sell_amount != input.sell_amount
        || parse_uint(&route.min_buy_amount)?.is_zero()
        || parse_uint(&route.buy_amount)? < parse_uint(&route.min_buy_amount)?
    {
        anyhow::bail!(ErrorKind::InvalidRoute);
    }
    let data = validate_transaction(input, &route.tx, route.deadline)?;
    if require_unified && chain.router.is_none() {
        anyhow::bail!(ErrorKind::RouterNotConfigured);
    }
    if chain.router.is_some() {
        let allowlisted = rules.iter().any(|rule| {
            rule.target == route.tx.to
                && rule.spender == route.spender
                && rule.selector.eq_ignore_ascii_case(&data[..10])
        });
        if !allowlisted {
            anyhow::bail!(ErrorKind::RouteNotAllowlisted);
        }
    }
    Ok(())
}

pub(crate) fn validate_transaction(
    input: &Input,
    tx: &Tx,
    deadline: Option<u64>,
) -> anyhow::Result<String> {
    if deadline.is_some_and(|deadline| deadline <= crate::domain::now_ms() / 1000) {
        anyhow::bail!(ErrorKind::QuoteExpired);
    }
    let route_value = parse_uint(&tx.value)?;
    let expected_value = if input.sell_token == NATIVE {
        parse_uint(&input.sell_amount)?
    } else {
        U256::ZERO
    };
    if route_value != expected_value {
        anyhow::bail!(ErrorKind::UnexpectedTransactionValue);
    }
    let data = parse_hex(&tx.data)?;
    if data.len() < 10 || data.len() > 262_146 {
        anyhow::bail!(ErrorKind::InvalidCalldata);
    }
    Ok(data)
}

pub fn swap_transaction(
    input: &Input,
    chain: &Chain,
    route: &Route,
    rules: &[Rule],
    min: Option<&str>,
) -> anyhow::Result<Tx> {
    validate_route(input, chain, route, rules, false)?;
    let Some(router) = chain.router else {
        return Ok(route.tx.clone());
    };
    let minimum = parse_uint(min.unwrap_or(&route.min_buy_amount))?;
    let provider_data = bytes_from_hex(&route.tx.data)?;
    let inner = executeCall {
        sellToken: input.sell_token,
        buyToken: input.buy_token,
        sellAmount: parse_uint(&input.sell_amount)?,
        minBuyAmount: minimum,
        target: route.tx.to,
        spender: route.spender,
        value: parse_uint(&route.tx.value)?,
        data: provider_data,
        deadline: route.deadline.map(U256::from).unwrap_or(U256::MAX),
    }
    .abi_encode();
    let outer = execCall {
        operator: router,
        token: input.sell_token,
        amount: parse_uint(&input.sell_amount)?,
        target: router,
        data: Bytes::from(inner),
    }
    .abi_encode();
    Ok(Tx {
        to: HOLDER,
        data: format!("0x{}", hex::encode(outer)),
        value: if input.sell_token == NATIVE {
            input.sell_amount.clone()
        } else {
            "0".into()
        },
    })
}

pub fn approval(token: Address, spender: Address, amount: U256) -> Tx {
    Tx {
        to: token,
        data: format!(
            "0x{}",
            hex::encode(approveCall { spender, amount }.abi_encode())
        ),
        value: "0".into(),
    }
}

pub fn balance_data(owner: Address) -> String {
    format!("0x{}", hex::encode(balanceOfCall { owner }.abi_encode()))
}

pub fn allowance_data(owner: Address, spender: Address) -> String {
    format!(
        "0x{}",
        hex::encode(allowanceCall { owner, spender }.abi_encode())
    )
}

fn bytes_from_hex(data: &str) -> anyhow::Result<Bytes> {
    let data = parse_hex(data)?;
    Ok(Bytes::from(
        hex::decode(&data[2..]).context(ErrorKind::InvalidCalldata)?,
    ))
}
