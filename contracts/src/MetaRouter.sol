// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

interface IAllowanceHolder {
    function transferFrom(address token, address owner, address recipient, uint256 amount) external returns (bool);
    function exec(address operator, address token, uint256 amount, address payable target, bytes calldata data)
        external
        payable
        returns (bytes memory);
}

/// @notice Exact-input execution with AllowanceHolder's ERC-2771 sender forwarding.
/// @dev The administrator must only allow audited provider target/spender/selector tuples.
///      Nonstandard taxed/rebasing tokens are not supported. No rescue or arbitrary-call admin API.
contract MetaRouter {
    address public constant NATIVE = 0xEeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE;
    address public immutable owner;
    address public immutable allowanceHolder;
    bool public paused;
    bool private entered;
    mapping(bytes32 => bool) public allowed;

    error Unauthorized();
    error Paused();
    error Reentrant();
    error InvalidInput();
    error Expired();
    error RouteNotAllowed();
    error TokenCallFailed();
    error BalanceInvariant();
    error InsufficientOutput();
    error NativeRefundFailed();

    event RoutePermission(address indexed target, address indexed spender, bytes4 selector, bool enabled);
    event PauseChanged(bool paused);
    event Executed(
        address indexed taker, address indexed sellToken, address indexed buyToken, uint256 sold, uint256 bought
    );

    constructor(address owner_, address allowanceHolder_) {
        if (owner_ == address(0) || allowanceHolder_.code.length == 0) revert InvalidInput();
        owner = owner_;
        allowanceHolder = allowanceHolder_;
    }

    modifier nonReentrant() {
        if (entered) revert Reentrant();
        entered = true;
        _;
        entered = false;
    }

    function setPaused(bool value) external {
        if (msg.sender != owner) revert Unauthorized();
        paused = value;
        emit PauseChanged(value);
    }

    function setAllowed(address target, address spender, bytes4 selector, bool enabled) external {
        if (msg.sender != owner) revert Unauthorized();
        if (
            enabled
                && (target.code.length == 0
                    || spender.code.length == 0
                    || target == address(this)
                    || spender == address(this))
        ) revert InvalidInput();
        // 0x allowance-holder routes call the holder a second time. In this nested execution
        // the Router is the token owner and Settler is the operator; its ephemeral allowance
        // is distinct from the outer (Router, taker, token) slot. Permit only exec here.
        if (
            enabled && (target == allowanceHolder || spender == allowanceHolder)
                && (target != allowanceHolder
                    || spender != allowanceHolder
                    || selector != IAllowanceHolder.exec.selector)
        ) {
            revert InvalidInput();
        }
        allowed[keccak256(abi.encode(target, spender, selector))] = enabled;
        emit RoutePermission(target, spender, selector, enabled);
    }

    function execute(
        address sellToken,
        address buyToken,
        uint256 sellAmount,
        uint256 minBuyAmount,
        address target,
        address spender,
        uint256 value,
        bytes calldata data,
        uint256 deadline
    ) external payable nonReentrant returns (uint256 boughtAmount) {
        if (msg.sender != allowanceHolder) revert Unauthorized();
        if (paused) revert Paused();
        if (block.timestamp > deadline) revert Expired();
        // Canonical ABI head (9 words), bytes length and padded bytes, then the forwarded sender.
        // This also prevents reading the final bytes of ordinary calldata as a sender.
        uint256 paddedDataLength = data.length;
        uint256 remainder = paddedDataLength % 32;
        if (remainder != 0) paddedDataLength += 32 - remainder;
        uint256 canonicalLength = 4 + 9 * 32 + 32 + paddedDataLength;
        if (msg.data.length != canonicalLength + 20) revert InvalidInput();
        address taker;
        assembly ("memory-safe") { taker := shr(96, calldataload(sub(calldatasize(), 20))) }
        if (
            taker == address(0) || taker == address(this) || taker == allowanceHolder || sellAmount == 0
                || minBuyAmount == 0 || buyToken.code.length == 0 || buyToken == sellToken || data.length < 4
                || target == sellToken || target == buyToken
        ) revert InvalidInput();
        if (!allowed[keccak256(abi.encode(target, spender, bytes4(data[:4])))]) revert RouteNotAllowed();

        bool nativeSell = sellToken == NATIVE;
        if (nativeSell) {
            if (msg.value != sellAmount || value > sellAmount) revert InvalidInput();
        } else if (sellToken.code.length == 0 || msg.value != 0 || value != 0) {
            revert InvalidInput();
        }

        uint256 nativeBefore = address(this).balance - msg.value;
        uint256 sellBefore = nativeSell ? 0 : _balance(sellToken, address(this));
        uint256 buyBefore = _balance(buyToken, address(this));
        uint256 takerBuyBefore = _balance(buyToken, taker);
        if (!nativeSell) {
            if (!IAllowanceHolder(allowanceHolder).transferFrom(sellToken, taker, address(this), sellAmount)) {
                revert TokenCallFailed();
            }
            if (_balance(sellToken, address(this)) != sellBefore + sellAmount) revert BalanceInvariant();
            _tokenCall(sellToken, abi.encodeWithSignature("approve(address,uint256)", spender, 0));
            _tokenCall(sellToken, abi.encodeWithSignature("approve(address,uint256)", spender, sellAmount));
        }

        (bool success, bytes memory result) = target.call{value: value}(data);
        if (!success) assembly ("memory-safe") { revert(add(result, 32), mload(result)) }
        if (!nativeSell) {
            _tokenCall(sellToken, abi.encodeWithSignature("approve(address,uint256)", spender, 0));
            uint256 sellAfter = _balance(sellToken, address(this));
            if (sellAfter < sellBefore) revert BalanceInvariant();
            if (sellAfter > sellBefore) _transfer(sellToken, taker, sellAfter - sellBefore);
        }
        uint256 buyAfter = _balance(buyToken, address(this));
        if (buyAfter < buyBefore) revert BalanceInvariant();
        if (buyAfter > buyBefore) _transfer(buyToken, taker, buyAfter - buyBefore);

        if (address(this).balance < nativeBefore) revert BalanceInvariant();
        uint256 nativeRefund = address(this).balance - nativeBefore;
        if (nativeRefund != 0) {
            (bool refunded,) = taker.call{value: nativeRefund}("");
            if (!refunded) revert NativeRefundFailed();
        }
        // Checked last, including any callbacks caused by token transfers and native refunds.
        uint256 finalBuy = _balance(buyToken, taker);
        if (finalBuy < takerBuyBefore || finalBuy - takerBuyBefore < minBuyAmount) revert InsufficientOutput();
        boughtAmount = finalBuy - takerBuyBefore;
        emit Executed(taker, sellToken, buyToken, sellAmount, boughtAmount);
    }

    function _balance(address token, address account) private view returns (uint256) {
        (bool ok, bytes memory result) = token.staticcall(abi.encodeWithSignature("balanceOf(address)", account));
        if (!ok || result.length != 32) revert TokenCallFailed();
        return abi.decode(result, (uint256));
    }

    function _transfer(address token, address to, uint256 amount) private {
        _tokenCall(token, abi.encodeWithSignature("transfer(address,uint256)", to, amount));
    }

    function _tokenCall(address token, bytes memory data) private {
        (bool ok, bytes memory result) = token.call(data);
        if (!ok || (result.length != 0 && (result.length != 32 || !abi.decode(result, (bool))))) {
            revert TokenCallFailed();
        }
    }

    receive() external payable {}
}
