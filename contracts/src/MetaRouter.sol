// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {IERC20} from "./IERC20.sol";

interface IAllowanceHolder {
    function transferFrom(address token, address owner, address recipient, uint256 amount) external returns (bool);
}

/// @notice Exact-input execution with AllowanceHolder's ERC-2771 sender forwarding.
/// @dev Permissionless routes; there is no administrator, pause, allowlist or asset recovery.
///      Only the current swap's input allowance, refunds and final output are checked.
///      Unrelated idle assets are not protected and must not be deposited here.
///      Use a trusted AllowanceHolder. Nonstandard taxed/rebasing tokens are not supported.
contract MetaRouter {
    address public constant NATIVE = 0xEeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE;
    address public immutable allowanceHolder;
    bool private entered;

    error Unauthorized();
    error Reentrant();
    error InvalidInput();
    error Expired();
    error TokenCallFailed();
    error BalanceInvariant();
    error InsufficientOutput();
    error NativeRefundFailed();

    /// @dev sold deducts only this Router's sell-asset refund, floored at zero; direct provider refunds are excluded.
    event Executed(
        address indexed sender,
        address receiver,
        address indexed sellToken,
        address indexed buyToken,
        uint256 sold,
        uint256 bought
    );

    constructor(address allowanceHolder_) {
        if (allowanceHolder_.code.length == 0) revert InvalidInput();
        allowanceHolder = allowanceHolder_;
    }

    modifier nonReentrant() {
        if (entered) revert Reentrant();
        entered = true;
        _;
        entered = false;
    }

    /// @notice Spend up to sellAmount and require a receiver buy-token increase of at least minBuyAmount.
    /// @dev Called via the configured Holder; any unspent input is refunded to the forwarded sender.
    function execute(
        address sellToken,
        address buyToken,
        address receiver,
        uint256 sellAmount,
        uint256 minBuyAmount,
        uint256 deadline,
        address spender,
        address target,
        uint256 value,
        bytes calldata data
    ) external payable nonReentrant returns (uint256 boughtAmount) {
        if (msg.sender != allowanceHolder) revert Unauthorized();
        if (block.timestamp > deadline) revert Expired();
        // Canonical ABI head (10 words), bytes length and padded bytes, then the forwarded sender.
        // This also prevents reading the final bytes of ordinary calldata as a sender.
        uint256 paddedDataLength = data.length;
        uint256 remainder = paddedDataLength % 32;
        if (remainder != 0) paddedDataLength += 32 - remainder;
        uint256 canonicalLength = 4 + 10 * 32 + 32 + paddedDataLength;
        if (msg.data.length != canonicalLength + 20) revert InvalidInput();
        address sender;
        assembly ("memory-safe") { sender := shr(96, calldataload(sub(calldatasize(), 20))) }
        if (
            sender == address(0) || sender == address(this) || sender == allowanceHolder || sellAmount == 0
                || receiver == address(0) || receiver == address(this) || receiver == allowanceHolder
                || receiver == NATIVE || minBuyAmount == 0 || buyToken.code.length == 0 || buyToken == sellToken
                || data.length < 4 || target == sellToken || target == buyToken || target == address(this)
                || spender == address(this) || target.code.length == 0 || spender.code.length == 0
        ) revert InvalidInput();

        bool nativeSell = sellToken == NATIVE;
        if (nativeSell) {
            if (msg.value != sellAmount || value > sellAmount) revert InvalidInput();
        } else if (sellToken.code.length == 0 || msg.value != 0 || value != 0) {
            revert InvalidInput();
        }

        uint256 nativeBefore = address(this).balance - msg.value;
        uint256 sellBefore = nativeSell ? 0 : _balance(sellToken, address(this));
        uint256 buyBefore = _balance(buyToken, address(this));
        uint256 receiverBuyBefore = _balance(buyToken, receiver);
        if (!nativeSell) {
            if (!IAllowanceHolder(allowanceHolder).transferFrom(sellToken, sender, address(this), sellAmount)) {
                revert TokenCallFailed();
            }
            if (_balance(sellToken, address(this)) != sellBefore + sellAmount) revert BalanceInvariant();
            _tokenCall(sellToken, abi.encodeCall(IERC20.approve, (spender, 0)));
            _tokenCall(sellToken, abi.encodeCall(IERC20.approve, (spender, sellAmount)));
        }

        (bool success, bytes memory result) = target.call{value: value}(data);
        if (!success) assembly ("memory-safe") { revert(add(result, 32), mload(result)) }
        uint256 sellRefund;
        if (!nativeSell) {
            _tokenCall(sellToken, abi.encodeCall(IERC20.approve, (spender, 0)));
            uint256 sellAfter = _balance(sellToken, address(this));
            if (sellAfter < sellBefore) revert BalanceInvariant();
            sellRefund = sellAfter - sellBefore;
            if (sellRefund != 0) _transfer(sellToken, sender, sellRefund);
        }
        uint256 buyAfter = _balance(buyToken, address(this));
        if (buyAfter < buyBefore) revert BalanceInvariant();
        if (buyAfter > buyBefore) _transfer(buyToken, receiver, buyAfter - buyBefore);

        if (address(this).balance < nativeBefore) revert BalanceInvariant();
        uint256 nativeRefund = address(this).balance - nativeBefore;
        if (nativeSell) sellRefund = nativeRefund;
        if (nativeRefund != 0) {
            (bool refunded,) = sender.call{value: nativeRefund}("");
            if (!refunded) revert NativeRefundFailed();
        }
        // Checked last, including any callbacks caused by token transfers and native refunds.
        uint256 finalBuy = _balance(buyToken, receiver);
        if (finalBuy < receiverBuyBefore || finalBuy - receiverBuyBefore < minBuyAmount) revert InsufficientOutput();
        boughtAmount = finalBuy - receiverBuyBefore;
        // Extra sell-asset receipts must not make event accounting revert an otherwise valid swap.
        uint256 soldAmount = sellRefund >= sellAmount ? 0 : sellAmount - sellRefund;
        emit Executed(sender, receiver, sellToken, buyToken, soldAmount, boughtAmount);
    }

    function _balance(address token, address account) private view returns (uint256) {
        (bool ok, bytes memory result) = token.staticcall(abi.encodeCall(IERC20.balanceOf, (account)));
        if (!ok || result.length != 32) revert TokenCallFailed();
        return abi.decode(result, (uint256));
    }

    function _transfer(address token, address to, uint256 amount) private {
        _tokenCall(token, abi.encodeCall(IERC20.transfer, (to, amount)));
    }

    function _tokenCall(address token, bytes memory data) private {
        (bool ok, bytes memory result) = token.call(data);
        if (!ok || (result.length != 0 && (result.length != 32 || !abi.decode(result, (bool))))) {
            revert TokenCallFailed();
        }
    }

    receive() external payable {}
}
