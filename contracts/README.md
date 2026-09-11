# MetaRouter

Local prototype. Run `forge test` and `forge fmt --check` from this directory. Solidity 0.8.25, Cancun EVM; no third-party Solidity dependencies. Deployment is intentionally not automated.

The constructor takes `(owner, allowanceHolder)`. `setAllowed(target, spender, selector, enabled)` configures an exact provider tuple; initial allowlist is empty. `setPaused` gates swaps. The configured holder must be the authentic, verified 0x AllowanceHolder deployment for the chain. An administrator should be a multisig. This source has not received an external audit.

Call `AllowanceHolder.exec(router, sellToken, amount, router, executeCalldata)` from the taker's wallet. The router only accepts calls from that holder and decodes its appended ERC-2771 sender. The holder's temporary operator permission must name the router. Approval goes to the holder, never this router. Native input uses `0xEeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE`; outer `msg.value` must equal `sellAmount`. ERC20 input requires zero native value. Buy token must be ERC20; output receiver is always the forwarded taker. Minimum output and deadline are mandatory.

The provider may send buy tokens to this router or directly to the taker. Only newly received router balances are transferred. Minimum output is checked using the taker's actual final balance delta, after refunds. Existing router sell, buy and native balances are preserved. Provider approvals are exact and reset after execution. Fee-on-transfer/rebasing assets are outside the supported token policy.

For 0x allowance-holder routes, the allowed outer tuple is `(allowanceHolder, allowanceHolder, exec.selector)`. The nested holder call sees the Router as its owner and a downstream Settler as its operator; it uses a different temporary allowance slot from the taker's outer call. The Router approves only this trade's exact input to the holder and clears that approval on completion. Other holder selector/target/spender combinations are rejected. Tests cover this nested flow, output minimum, overspending, historical balances and both temporary allowance slots.

The test holder models sender forwarding and owner/operator/token allowance binding and cleanup. It is a test double, not audited production AllowanceHolder bytecode. Production compatibility should additionally be verified against the selected chain's actual holder on a mainnet fork before deployment.
