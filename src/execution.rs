use crate::domain::{Input, NATIVE, Route, RouterDeployment, Tx, parse_uint};
use crate::error::ErrorKind;
use alloy_primitives::{Address, Bytes, U256};
use alloy_sol_types::{SolCall, sol};

sol! {
    function approve(address spender, uint256 amount) returns (bool);
    function balanceOf(address owner) view returns (uint256);
    function allowance(address owner, address spender) view returns (uint256);
    function allowanceHolder() view returns (address);
    function execute(address sellToken, address buyToken, address receiver, uint256 sellAmount, uint256 minBuyAmount, uint256 deadline, address spender, address target, uint256 value, bytes data) payable returns (uint256);
    function exec(address operator, address token, uint256 amount, address target, bytes data) payable returns (bytes);
}

/// Checked once at the provider/competition boundary, immutable through encoding and simulation.
#[derive(Debug)]
pub struct ValidatedRoute(Route);

impl std::ops::Deref for ValidatedRoute {
    type Target = Route;
    fn deref(&self) -> &Route {
        &self.0
    }
}

impl ValidatedRoute {
    pub fn into_inner(self) -> Route {
        self.0
    }
}

pub fn validate_route(input: &Input, route: Route) -> anyhow::Result<ValidatedRoute> {
    if route.sell_amount != input.sell_amount
        || parse_uint(&route.min_buy_amount)?.is_zero()
        || parse_uint(&route.buy_amount)? < parse_uint(&route.min_buy_amount)?
        || route.tx.to == Address::ZERO
        || (input.sell_token != NATIVE && route.spender == Address::ZERO)
    {
        anyhow::bail!(ErrorKind::InvalidRoute);
    }
    validate_transaction(input, &route.tx, route.deadline)?;
    Ok(ValidatedRoute(route))
}

pub fn validate_deadline(deadline: Option<u64>) -> anyhow::Result<()> {
    if deadline.is_some_and(|deadline| deadline <= crate::domain::now_ms() / 1000) {
        anyhow::bail!(ErrorKind::QuoteExpired);
    }
    Ok(())
}

pub(crate) fn validate_transaction(
    input: &Input,
    tx: &Tx,
    deadline: Option<u64>,
) -> anyhow::Result<()> {
    validate_deadline(deadline)?;
    let route_value = parse_uint(&tx.value)?;
    let expected_value = if input.sell_token == NATIVE {
        parse_uint(&input.sell_amount)?
    } else {
        U256::ZERO
    };
    if route_value != expected_value {
        anyhow::bail!(ErrorKind::UnexpectedTransactionValue);
    }
    if !(4..=131_072).contains(&tx.data.len()) {
        anyhow::bail!(ErrorKind::InvalidCalldata);
    }
    Ok(())
}

pub fn swap_transaction(
    input: &Input,
    router: RouterDeployment,
    route: &ValidatedRoute,
) -> anyhow::Result<Tx> {
    // Wall time can advance after boundary validation and shared RPC preparation.
    validate_deadline(route.deadline)?;
    let inner = executeCall {
        sellToken: input.sell_token,
        buyToken: input.buy_token,
        receiver: input.taker,
        sellAmount: parse_uint(&input.sell_amount)?,
        minBuyAmount: parse_uint(&route.min_buy_amount)?,
        target: route.tx.to,
        spender: route.spender,
        value: parse_uint(&route.tx.value)?,
        data: route.tx.data.clone(),
        deadline: route.deadline.map(U256::from).unwrap_or(U256::MAX),
    }
    .abi_encode();
    let outer = execCall {
        operator: router.address,
        token: input.sell_token,
        amount: parse_uint(&input.sell_amount)?,
        target: router.address,
        data: Bytes::from(inner),
    }
    .abi_encode();
    Ok(Tx {
        to: router.holder,
        data: outer.into(),
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
        data: approveCall { spender, amount }.abi_encode().into(),
        value: "0".into(),
    }
}

pub fn balance_data(owner: Address) -> Bytes {
    balanceOfCall { owner }.abi_encode().into()
}

pub fn allowance_data(owner: Address, spender: Address) -> Bytes {
    allowanceCall { owner, spender }.abi_encode().into()
}
