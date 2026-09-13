use crate::domain::{Chain, Fault, HOLDER, Input, NATIVE, Route, Rule, Tx, parse_hex, parse_uint};
use alloy_primitives::{Address, Bytes, U256};
use alloy_sol_types::{SolCall, sol};

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
) -> Result<(), Fault> {
    if route.sell_amount != input.sell_amount
        || parse_uint(&route.min_buy_amount)?.is_zero()
        || parse_uint(&route.buy_amount)? < parse_uint(&route.min_buy_amount)?
    {
        return Err(Fault::with_status("INVALID_ROUTE", 502));
    }
    if route.expires_at <= crate::domain::now_ms() {
        return Err(Fault::with_status("QUOTE_EXPIRED", 409));
    }
    let route_value = parse_uint(&route.tx.value)?;
    let expected_value = if input.sell_token == NATIVE {
        parse_uint(&input.sell_amount)?
    } else {
        U256::ZERO
    };
    if route_value != expected_value {
        return Err(Fault::with_status("UNEXPECTED_TRANSACTION_VALUE", 502));
    }
    let data = parse_hex(&route.tx.data)?;
    if data.len() < 10 || data.len() > 262_146 {
        return Err(Fault::with_status("INVALID_CALLDATA", 502));
    }
    if require_unified && chain.router.is_none() {
        return Err(Fault::with_status("ROUTER_NOT_CONFIGURED", 503));
    }
    if let Some(_router) = chain.router {
        let allowlisted = rules.iter().any(|rule| {
            rule.target == route.tx.to
                && rule.spender == route.spender
                && rule.selector.eq_ignore_ascii_case(&data[..10])
        });
        if !allowlisted {
            return Err(Fault::with_status("ROUTE_NOT_ALLOWLISTED", 422));
        }
    }
    Ok(())
}

pub fn swap_transaction(
    input: &Input,
    chain: &Chain,
    route: &Route,
    rules: &[Rule],
    min: Option<&str>,
) -> Result<Tx, Fault> {
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
        deadline: U256::from(route.expires_at / 1000),
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

fn bytes_from_hex(data: &str) -> Result<Bytes, Fault> {
    let data = parse_hex(data)?;
    Ok(Bytes::from(hex::decode(&data[2..]).map_err(|_| {
        Fault::with_status("INVALID_CALLDATA", 502)
    })?))
}
