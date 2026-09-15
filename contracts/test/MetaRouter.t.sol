// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {MetaRouter} from "../src/MetaRouter.sol";
import {IERC20} from "../src/IERC20.sol";

interface Vm {
    function deal(address account, uint256 amount) external;
    function prank(address sender) external;
    function expectRevert(bytes4 selector) external;
    function expectRevert() external;
    function expectEmit(bool checkTopic1, bool checkTopic2, bool checkTopic3, bool checkData, address emitter) external;
    function warp(uint256 timestamp) external;
    function mockCall(address callee, bytes calldata data, bytes calldata returnData) external;
}

contract MockToken {
    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;
    bool public failApprove;
    bool public failTransfer;

    function mint(address to, uint256 amount) external {
        balanceOf[to] += amount;
    }

    function setFailures(bool approvals, bool transfers) external {
        failApprove = approvals;
        failTransfer = transfers;
    }

    function approve(address spender, uint256 amount) external returns (bool) {
        if (failApprove) return false;
        allowance[msg.sender][spender] = amount;
        return true;
    }

    function transfer(address to, uint256 amount) external returns (bool) {
        if (failTransfer) return false;
        balanceOf[msg.sender] -= amount;
        balanceOf[to] += amount;
        return true;
    }

    function transferFrom(address from, address to, uint256 amount) external returns (bool) {
        if (failTransfer) return false;
        allowance[from][msg.sender] -= amount;
        balanceOf[from] -= amount;
        balanceOf[to] += amount;
        return true;
    }
}

/// @dev Test-only model of AllowanceHolder's operator/owner/token ephemeral allowance.
///      It appends the actual caller, never an owner supplied as an argument.
contract MockAllowanceHolder {
    mapping(bytes32 => uint256) public temporaryAllowance;

    function exec(address operator, address token, uint256 amount, address payable target, bytes calldata data)
        external
        payable
        returns (bytes memory result)
    {
        // Production holder rejects ERC20 targets to prevent spending its permanent allowances.
        (bool isToken, bytes memory balance) = target.staticcall(abi.encodeCall(IERC20.balanceOf, (address(0xdead))));
        require(!isToken || balance.length < 32, "ERC20 target");
        bytes32 key = keccak256(abi.encode(operator, msg.sender, token));
        temporaryAllowance[key] = amount;
        bool ok;
        (ok, result) = target.call{value: msg.value}(bytes.concat(data, bytes20(msg.sender)));
        if (!ok) assembly ("memory-safe") { revert(add(result, 32), mload(result)) }
        temporaryAllowance[key] = 0;
    }

    function transferFrom(address token, address owner, address recipient, uint256 amount) external returns (bool) {
        bytes32 key = keccak256(abi.encode(msg.sender, owner, token));
        temporaryAllowance[key] -= amount;
        require(MockToken(token).transferFrom(owner, recipient, amount), "transfer false");
        return true;
    }
}

contract MockProvider {
    function swapWithSellRefund(address sell, address buy, uint256 spend, uint256 refund, address refundRecipient)
        external
    {
        require(MockToken(sell).transferFrom(msg.sender, address(this), spend));
        require(MockToken(sell).transfer(refundRecipient, refund));
        MockToken(buy).mint(msg.sender, 1);
    }

    function swapViaSpender(
        MockSpender spender,
        address sell,
        address buy,
        uint256 spend,
        uint256 output,
        address recipient
    ) external {
        spender.take(sell, msg.sender, spend);
        MockToken(buy).mint(recipient, output);
    }

    function takeOtherWalletTokens(MockAllowanceHolder holder, address token, address victim) external payable {
        holder.transferFrom(token, victim, address(this), 1);
    }

    function swap(address sell, address buy, uint256 spend, uint256 output, address recipient, uint256 refund)
        external
        payable
    {
        if (msg.value == 0) require(MockToken(sell).transferFrom(msg.sender, address(this), spend));
        MockToken(buy).mint(recipient, output);
        if (refund > 0) {
            (bool ok,) = msg.sender.call{value: refund}("");
            require(ok);
        }
    }

    function reenter(MockAllowanceHolder holder, MetaRouter router, bytes calldata execution) external {
        holder.exec(address(router), address(0), 0, payable(address(router)), execution);
    }
}

contract MockSpender {
    function take(address token, address owner, uint256 amount) external {
        require(MockToken(token).transferFrom(owner, address(this), amount));
    }
}

/// @dev A route entry may expose balanceOf without being rejected as an ERC20 target.
contract MockBalanceReportingProvider is MockProvider {
    function balanceOf(address) external pure returns (uint256) {
        return 0;
    }
}

/// @dev Minimal nested-holder consumer: sender is appended by the holder and used as
///      the owner when exercising the inner ephemeral allowance, as in Settler.
contract MockSettler {
    MockAllowanceHolder private immutable holder;

    constructor(MockAllowanceHolder holder_) {
        holder = holder_;
    }

    function swap(address sell, address buy, uint256 spend, uint256 output, address recipient) external {
        require(msg.sender == address(holder));
        address forwardedOwner;
        assembly ("memory-safe") { forwardedOwner := shr(96, calldataload(sub(calldatasize(), 20))) }
        holder.transferFrom(sell, forwardedOwner, address(this), spend);
        MockToken(buy).mint(recipient, output);
    }
}

contract MetaRouterTest {
    Vm private constant vm = Vm(address(uint160(uint256(keccak256("hevm cheat code")))));
    address private constant SENDER = address(0xBEEF);
    address private constant RECEIVER = address(0xCAFE);
    MetaRouter private router;
    MockAllowanceHolder private holder;
    MockToken private sell;
    MockToken private buy;
    MockProvider private provider;

    function setUp() public {
        holder = new MockAllowanceHolder();
        router = new MetaRouter(address(holder));
        sell = new MockToken();
        buy = new MockToken();
        provider = new MockProvider();
        sell.mint(SENDER, 1000 ether);
        vm.prank(SENDER);
        sell.approve(address(holder), type(uint256).max);
        vm.deal(SENDER, 1000 ether);
    }

    function _data(uint256 spend, uint256 output, address recipient, uint256 refund)
        private
        view
        returns (bytes memory)
    {
        return abi.encodeCall(MockProvider.swap, (address(sell), address(buy), spend, output, recipient, refund));
    }

    function _execution(
        address sellToken,
        uint256 amount,
        uint256 min,
        uint256 value,
        bytes memory data,
        uint256 deadline
    ) private view returns (bytes memory) {
        return _executionTo(SENDER, sellToken, amount, min, value, data, deadline);
    }

    function _executionTo(
        address receiver,
        address sellToken,
        uint256 amount,
        uint256 min,
        uint256 value,
        bytes memory data,
        uint256 deadline
    ) private view returns (bytes memory) {
        return abi.encodeCall(
            MetaRouter.execute,
            (
                sellToken,
                address(buy),
                receiver,
                amount,
                min,
                deadline,
                address(provider),
                address(provider),
                value,
                data
            )
        );
    }

    function _run(uint256 amount, uint256 minimum, uint256 spend, uint256 output) private returns (uint256) {
        bytes memory execution = _execution(
            address(sell), amount, minimum, 0, _data(spend, output, address(router), 0), block.timestamp + 60
        );
        vm.prank(SENDER);
        bytes memory result = holder.exec(address(router), address(sell), amount, payable(address(router)), execution);
        // expectRevert consumes the revert and returns empty bytes to this helper.
        return result.length == 0 ? 0 : abi.decode(result, (uint256));
    }

    function testSuccessfulSwapAndAllowanceCleanup() public {
        _expectExecuted(address(sell), 90, 85);
        require(_run(100, 80, 90, 85) == 85);
        require(sell.balanceOf(SENDER) == 1000 ether - 90);
        require(buy.balanceOf(SENDER) == 85);
        require(sell.allowance(address(router), address(provider)) == 0);
        require(holder.temporaryAllowance(keccak256(abi.encode(address(router), SENDER, address(sell)))) == 0);
    }

    function testReceiverGetsOutputAndSenderGetsSellRefund() public {
        sell.mint(address(router), 123);
        buy.mint(address(router), 456);
        bytes memory execution =
            _executionTo(RECEIVER, address(sell), 100, 50, 0, _data(90, 60, address(router), 0), block.timestamp + 60);
        vm.expectEmit(true, true, true, true, address(router));
        emit MetaRouter.Executed(SENDER, RECEIVER, address(sell), address(buy), 90, 60);
        vm.prank(SENDER);
        bytes memory result = holder.exec(address(router), address(sell), 100, payable(address(router)), execution);
        require(abi.decode(result, (uint256)) == 60);
        require(sell.balanceOf(SENDER) == 1000 ether - 90);
        require(sell.balanceOf(RECEIVER) == 0);
        require(buy.balanceOf(RECEIVER) == 60);
        require(buy.balanceOf(SENDER) == 0);
        require(sell.balanceOf(address(router)) == 123);
        require(buy.balanceOf(address(router)) == 456);
        require(sell.allowance(address(router), address(provider)) == 0);
    }

    function testDirectProviderReceiverSupported() public {
        bytes memory execution =
            _executionTo(RECEIVER, address(sell), 100, 50, 0, _data(90, 60, RECEIVER, 0), block.timestamp + 60);
        vm.prank(SENDER);
        bytes memory result = holder.exec(address(router), address(sell), 100, payable(address(router)), execution);
        require(abi.decode(result, (uint256)) == 60);
        require(buy.balanceOf(RECEIVER) == 60);
        require(buy.balanceOf(SENDER) == 0);
        require(sell.balanceOf(SENDER) == 1000 ether - 90);
    }

    function testOutputToSenderCannotMeetReceiverMinimum() public {
        buy.mint(RECEIVER, 10000);
        bytes memory execution =
            _executionTo(RECEIVER, address(sell), 100, 50, 0, _data(100, 60, SENDER, 0), block.timestamp + 60);
        vm.prank(SENDER);
        vm.expectRevert(MetaRouter.InsufficientOutput.selector);
        holder.exec(address(router), address(sell), 100, payable(address(router)), execution);
        require(buy.balanceOf(RECEIVER) == 10000);
        require(buy.balanceOf(SENDER) == 0);
        require(sell.balanceOf(SENDER) == 1000 ether);
        require(sell.allowance(address(router), address(provider)) == 0);
    }

    function testFuzzReceiverMinimum(uint64 output, uint64 rawMinimum, bool direct) public {
        uint256 minimum = uint256(rawMinimum) + 1;
        buy.mint(RECEIVER, 10000);
        buy.mint(SENDER, 20000);
        bytes memory execution = _executionTo(
            RECEIVER,
            address(sell),
            100,
            minimum,
            0,
            _data(90, output, direct ? RECEIVER : address(router), 0),
            block.timestamp + 60
        );
        vm.prank(SENDER);
        if (output < minimum) vm.expectRevert(MetaRouter.InsufficientOutput.selector);
        bytes memory result = holder.exec(address(router), address(sell), 100, payable(address(router)), execution);
        if (output < minimum) {
            require(sell.balanceOf(SENDER) == 1000 ether);
            require(buy.balanceOf(RECEIVER) == 10000);
        } else {
            require(abi.decode(result, (uint256)) == output);
            require(sell.balanceOf(SENDER) == 1000 ether - 90);
            require(buy.balanceOf(RECEIVER) == 10000 + uint256(output));
        }
        require(buy.balanceOf(SENDER) == 20000);
        require(sell.allowance(address(router), address(provider)) == 0);
    }

    function testInvalidReceiversRejected() public {
        address[4] memory receivers = [address(0), address(router), address(holder), router.NATIVE()];
        for (uint256 i; i < receivers.length; ++i) {
            bytes memory execution = _executionTo(
                receivers[i], address(sell), 100, 1, 0, _data(100, 1, address(router), 0), block.timestamp + 60
            );
            vm.prank(SENDER);
            vm.expectRevert(MetaRouter.InvalidInput.selector);
            holder.exec(address(router), address(sell), 100, payable(address(router)), execution);
        }
        require(sell.balanceOf(SENDER) == 1000 ether);
    }

    function testNativeRefundGoesToSenderNotReceiver() public {
        // An ERC20 receiver does not have to accept native currency.
        RejectNative receiver = new RejectNative();
        address native = router.NATIVE();
        vm.deal(address(router), 7 ether);
        bytes memory execution = _executionTo(
            address(receiver),
            native,
            1 ether,
            50,
            0.8 ether,
            _data(0, 60, address(router), 0.2 ether),
            block.timestamp + 60
        );
        vm.expectEmit(true, true, true, true, address(router));
        emit MetaRouter.Executed(SENDER, address(receiver), native, address(buy), 0.6 ether, 60);
        vm.prank(SENDER);
        holder.exec{value: 1 ether}(address(router), native, 1 ether, payable(address(router)), execution);
        require(SENDER.balance == 999.4 ether);
        require(address(receiver).balance == 0);
        require(buy.balanceOf(address(receiver)) == 60);
        require(buy.balanceOf(SENDER) == 0);
        require(address(router).balance == 7 ether);
    }

    function testNestedHolderSupportsReceiver() public {
        _nestedTo(RECEIVER, 80, 60, 50);
        require(sell.balanceOf(SENDER) == 1000 ether - 80);
        require(buy.balanceOf(RECEIVER) == 60);
        require(buy.balanceOf(SENDER) == 0);
        require(sell.allowance(address(router), address(holder)) == 0);
    }

    function testReceiverMinimumCheckedAfterSenderRefundCallback() public {
        SwapCallback sender = new SwapCallback();
        sender.configure(address(buy), abi.encodeCall(IERC20.transferFrom, (RECEIVER, SENDER, 1)));
        buy.mint(RECEIVER, 10);
        vm.prank(RECEIVER);
        buy.approve(address(sender), 1);
        address native = router.NATIVE();
        vm.deal(address(sender), 1 ether);
        bytes memory execution = _executionTo(
            RECEIVER, native, 1 ether, 1, 1 ether, _data(0, 1, address(router), 0.4 ether), block.timestamp + 60
        );
        vm.prank(address(sender));
        vm.expectRevert(MetaRouter.InsufficientOutput.selector);
        holder.exec{value: 1 ether}(address(router), native, 1 ether, payable(address(router)), execution);
        require(buy.balanceOf(RECEIVER) == 10);
        require(buy.balanceOf(SENDER) == 0);
        require(buy.allowance(RECEIVER, address(sender)) == 1);
        require(address(sender).balance == 1 ether);
        require(address(provider).balance == 0);
    }

    function testNoReturnTokensSupportedWithReceiver() public {
        NoReturnToken input = new NoReturnToken();
        NoReturnToken output = new NoReturnToken();
        sell = MockToken(address(input));
        buy = MockToken(address(output));
        input.mint(SENDER, 100);
        vm.prank(SENDER);
        input.approve(address(holder), 100);
        bytes memory execution =
            _executionTo(RECEIVER, address(input), 100, 50, 0, _data(90, 60, address(router), 0), block.timestamp + 60);
        vm.prank(SENDER);
        holder.exec(address(router), address(input), 100, payable(address(router)), execution);
        require(input.balanceOf(SENDER) == 10);
        require(output.balanceOf(RECEIVER) == 60);
        require(input.allowance(address(router), address(provider)) == 0);
    }

    function testMalformedERC20ResultsRejected() public {
        // Type-safe encoding must retain the existing optional-return validation.
        vm.mockCall(address(sell), abi.encodeCall(IERC20.approve, (address(provider), 0)), hex"01");
        vm.expectRevert(MetaRouter.TokenCallFailed.selector);
        _run(100, 1, 100, 1);
        vm.mockCall(address(buy), abi.encodeCall(IERC20.balanceOf, (RECEIVER)), hex"01");
        bytes memory execution =
            _executionTo(RECEIVER, address(sell), 100, 1, 0, _data(100, 1, address(router), 0), block.timestamp + 60);
        vm.prank(SENDER);
        vm.expectRevert(MetaRouter.TokenCallFailed.selector);
        holder.exec(address(router), address(sell), 100, payable(address(router)), execution);
        require(sell.balanceOf(SENDER) == 1000 ether);
    }

    function _expectExecuted(address sellToken, uint256 sold, uint256 bought) private {
        vm.expectEmit(true, true, true, true, address(router));
        emit MetaRouter.Executed(SENDER, SENDER, sellToken, address(buy), sold, bought);
    }

    function testERC20SoldWithoutRefundAndWithFullRefund() public {
        _expectExecuted(address(sell), 100, 1);
        _run(100, 1, 100, 1);
        _expectExecuted(address(sell), 0, 1);
        _run(100, 1, 0, 1);
        require(sell.balanceOf(SENDER) == 1000 ether - 100);
    }

    function testFuzzERC20SoldWithAdditionalRefund(uint64 rawAmount, uint64 rawSpend, uint64 refund) public {
        uint256 amount = uint256(rawAmount) + 1;
        uint256 spend = uint256(rawSpend) % (amount + 1);
        sell.mint(address(router), 123);
        sell.mint(address(provider), refund);
        bytes memory execution = _execution(
            address(sell),
            amount,
            1,
            0,
            abi.encodeCall(
                MockProvider.swapWithSellRefund, (address(sell), address(buy), spend, refund, address(router))
            ),
            block.timestamp + 60
        );
        _expectExecuted(address(sell), refund >= spend ? 0 : spend - refund, 1);
        vm.prank(SENDER);
        bytes memory result = holder.exec(address(router), address(sell), amount, payable(address(router)), execution);
        require(abi.decode(result, (uint256)) == 1);
        require(sell.balanceOf(SENDER) == 1000 ether - spend + refund);
        require(sell.balanceOf(address(router)) == 123);
        require(sell.allowance(address(router), address(provider)) == 0);
    }

    function testERC20SoldExcludesDirectProviderRefundToSender() public {
        bytes memory execution = _execution(
            address(sell),
            100,
            1,
            0,
            abi.encodeCall(MockProvider.swapWithSellRefund, (address(sell), address(buy), 90, 10, SENDER)),
            block.timestamp + 60
        );
        // Router refunds 10; a separate provider refund of 10 is outside the event's accounting.
        _expectExecuted(address(sell), 90, 1);
        vm.prank(SENDER);
        holder.exec(address(router), address(sell), 100, payable(address(router)), execution);
        require(sell.balanceOf(SENDER) == 1000 ether - 80);
    }

    function testERC20SoldIgnoresNativeRefund() public {
        vm.deal(address(provider), 1 ether);
        bytes memory execution =
            _execution(address(sell), 100, 1, 0, _data(90, 1, address(router), 1 ether), block.timestamp + 60);
        _expectExecuted(address(sell), 90, 1);
        vm.prank(SENDER);
        holder.exec(address(router), address(sell), 100, payable(address(router)), execution);
        require(sell.balanceOf(SENDER) == 1000 ether - 90);
        require(SENDER.balance == 1001 ether);
    }

    function testNativeSoldIncludesUnforwardedInputAndProviderRefund() public {
        address native = router.NATIVE();
        bytes memory execution =
            _execution(native, 1 ether, 1, 0.6 ether, _data(0, 1, address(router), 0.2 ether), block.timestamp + 60);
        _expectExecuted(native, 0.4 ether, 1);
        vm.prank(SENDER);
        holder.exec{value: 1 ether}(address(router), native, 1 ether, payable(address(router)), execution);
        require(SENDER.balance == 999.6 ether);
    }

    function testNativeSoldWithoutRefundFullRefundAndExcessRefund() public {
        address native = router.NATIVE();
        vm.deal(address(provider), 1 ether);
        for (uint256 i; i < 3; ++i) {
            uint256 refund = i == 0 ? 0 : i == 1 ? 1 ether : 1 ether + 1;
            bytes memory execution =
                _execution(native, 1 ether, 1, 1 ether, _data(0, 1, address(router), refund), block.timestamp + 60);
            _expectExecuted(native, i == 0 ? 1 ether : 0, 1);
            vm.prank(SENDER);
            holder.exec{value: 1 ether}(address(router), native, 1 ether, payable(address(router)), execution);
        }
        require(SENDER.balance == 999 ether + 1);
        require(address(router).balance == 0);
    }

    function testExecuteSelectorUsesNewParameterOrder() public pure {
        require(
            MetaRouter.execute.selector
                == bytes4(
                    keccak256("execute(address,address,address,uint256,uint256,uint256,address,address,uint256,bytes)")
                )
        );
    }

    function testFuzzRefundPreservesHistoricalBalances(
        uint96 historicSell,
        uint96 historicBuy,
        uint96 historicNative,
        uint64 rawAmount,
        uint64 rawSpend
    ) public {
        uint256 amount = uint256(rawAmount) + 1;
        uint256 spend = uint256(rawSpend) % (amount + 1);
        sell.mint(address(router), historicSell);
        buy.mint(address(router), historicBuy);
        vm.deal(address(router), historicNative);
        _expectExecuted(address(sell), spend, 17);
        _run(amount, 1, spend, 17);
        require(sell.balanceOf(address(router)) == historicSell);
        require(buy.balanceOf(address(router)) == historicBuy);
        require(address(router).balance == historicNative);
        require(sell.balanceOf(SENDER) == 1000 ether - spend);
        require(buy.balanceOf(SENDER) == 17);
        require(sell.allowance(address(router), address(provider)) == 0);
    }

    function testFuzzMinimumOutput(uint64 output, uint64 rawMinimum) public {
        uint256 minimum = uint256(rawMinimum) + 1;
        if (output < minimum) vm.expectRevert(MetaRouter.InsufficientOutput.selector);
        _run(100, minimum, 100, output);
        if (output < minimum) {
            require(sell.balanceOf(SENDER) == 1000 ether);
            require(buy.balanceOf(SENDER) == 0);
        }
    }

    function testDirectProviderRecipientSupported() public {
        bytes memory execution = _execution(address(sell), 100, 50, 0, _data(100, 60, SENDER, 0), block.timestamp + 60);
        vm.prank(SENDER);
        holder.exec(address(router), address(sell), 100, payable(address(router)), execution);
        require(buy.balanceOf(SENDER) == 60);
    }

    function testNativeRefundPreservesHistory() public {
        address native = router.NATIVE();
        vm.deal(address(router), 7 ether);
        bytes memory execution = _execution(
            router.NATIVE(), 1 ether, 50, 1 ether, _data(0, 60, address(router), 0.4 ether), block.timestamp + 60
        );
        vm.prank(SENDER);
        holder.exec{value: 1 ether}(address(router), native, 1 ether, payable(address(router)), execution);
        require(address(router).balance == 7 ether);
        require(SENDER.balance == 999.4 ether);
        require(buy.balanceOf(SENDER) == 60);
    }

    function testWrongNativeValue() public {
        address native = router.NATIVE();
        bytes memory execution =
            _execution(router.NATIVE(), 1 ether, 1, 1 ether, _data(0, 2, address(router), 0), block.timestamp + 60);
        vm.prank(SENDER);
        vm.expectRevert(MetaRouter.InvalidInput.selector);
        holder.exec{value: 0.5 ether}(address(router), native, 1 ether, payable(address(router)), execution);
    }

    function testFuzzNativeRefund(uint64 rawAmount, uint64 rawRefund, uint96 history) public {
        address native = router.NATIVE();
        uint256 amount = uint256(rawAmount) + 1;
        uint256 refund = rawRefund;
        vm.deal(address(router), history);
        vm.deal(address(provider), refund);
        bytes memory execution =
            _execution(native, amount, 1, amount, _data(0, 1, address(router), refund), block.timestamp + 60);
        _expectExecuted(native, refund >= amount ? 0 : amount - refund, 1);
        vm.prank(SENDER);
        holder.exec{value: amount}(address(router), native, amount, payable(address(router)), execution);
        require(address(router).balance == history);
        require(SENDER.balance == 1000 ether - amount + refund);
        require(buy.balanceOf(SENDER) == 1);
    }

    function testERC20RejectsValue() public {
        bytes memory execution =
            _execution(address(sell), 1, 1, 1, _data(1, 2, address(router), 0), block.timestamp + 60);
        vm.prank(SENDER);
        vm.expectRevert(MetaRouter.InvalidInput.selector);
        holder.exec(address(router), address(sell), 1, payable(address(router)), execution);
    }

    function testCannotOverspendEvenWithHistoricalBalance() public {
        sell.mint(address(router), 1000);
        vm.expectRevert();
        _run(100, 1, 101, 50);
        require(sell.balanceOf(address(router)) == 1000);
    }

    function testDirectCallAndForgedSuffixRejected() public {
        bytes memory execution =
            _execution(address(sell), 100, 1, 0, _data(100, 10, address(router), 0), block.timestamp + 60);
        vm.prank(SENDER);
        (bool ok,) = address(router).call(bytes.concat(execution, bytes20(SENDER)));
        require(!ok);
    }

    function testCannotForgeAnotherOwnerThroughHolder() public {
        address attacker = address(0xBAD);
        bytes memory execution =
            _execution(address(sell), 100, 1, 0, _data(100, 10, address(router), 0), block.timestamp + 60);
        vm.prank(attacker);
        vm.expectRevert(MetaRouter.InvalidInput.selector);
        holder.exec(
            address(router), address(sell), 100, payable(address(router)), bytes.concat(execution, bytes20(SENDER))
        );
        require(sell.balanceOf(SENDER) == 1000 ether);
    }

    function testWrongOperatorCannotUseEphemeralAllowance() public {
        bytes memory execution =
            _execution(address(sell), 100, 1, 0, _data(100, 10, address(router), 0), block.timestamp + 60);
        vm.prank(SENDER);
        vm.expectRevert();
        holder.exec(address(provider), address(sell), 100, payable(address(router)), execution);
    }

    function testNewTargetNeedsNoRegistration() public {
        MockProvider fresh = new MockProvider();
        bytes memory execution = abi.encodeCall(
            MetaRouter.execute,
            (
                address(sell),
                address(buy),
                SENDER,
                100,
                50,
                block.timestamp + 60,
                address(fresh),
                address(fresh),
                0,
                _data(90, 60, address(router), 0)
            )
        );
        vm.prank(SENDER);
        bytes memory result = holder.exec(address(router), address(sell), 100, payable(address(router)), execution);
        require(abi.decode(result, (uint256)) == 60);
        require(sell.balanceOf(SENDER) == 1000 ether - 90);
        require(buy.balanceOf(SENDER) == 60);
        require(sell.allowance(address(router), address(fresh)) == 0);
    }

    function testNewSelectorAndIndependentSpenderNeedNoRegistration() public {
        MockSpender spender = new MockSpender();
        bytes memory data =
            abi.encodeCall(MockProvider.swapViaSpender, (spender, address(sell), address(buy), 90, 60, SENDER));
        bytes memory execution = abi.encodeCall(
            MetaRouter.execute,
            (
                address(sell),
                address(buy),
                SENDER,
                100,
                50,
                block.timestamp + 60,
                address(spender),
                address(provider),
                0,
                data
            )
        );
        vm.prank(SENDER);
        holder.exec(address(router), address(sell), 100, payable(address(router)), execution);
        require(sell.balanceOf(SENDER) == 1000 ether - 90);
        require(sell.balanceOf(address(spender)) == 90);
        require(buy.balanceOf(SENDER) == 60);
        require(sell.allowance(address(router), address(spender)) == 0);
        require(sell.allowance(address(router), address(provider)) == 0);
    }

    function testRouteCannotUseOtherWalletsPermanentHolderApproval() public {
        MockToken unrelated = new MockToken();
        address victim = address(0xCAFE);
        unrelated.mint(victim, 100);
        vm.prank(victim);
        unrelated.approve(address(holder), type(uint256).max);
        address native = router.NATIVE();
        bytes memory execution = _execution(
            native,
            1 ether,
            1,
            1 ether,
            abi.encodeCall(MockProvider.takeOtherWalletTokens, (holder, address(unrelated), victim)),
            block.timestamp + 60
        );
        vm.prank(SENDER);
        vm.expectRevert();
        holder.exec{value: 1 ether}(address(router), native, 1 ether, payable(address(router)), execution);
        require(unrelated.balanceOf(victim) == 100);
        require(unrelated.balanceOf(address(provider)) == 0);
        require(unrelated.allowance(victim, address(holder)) == type(uint256).max);
        require(SENDER.balance == 1000 ether);
    }

    function testConstructorRequiresHolderCode() public {
        vm.expectRevert(MetaRouter.InvalidInput.selector);
        new MetaRouter(address(0));
        vm.expectRevert(MetaRouter.InvalidInput.selector);
        new MetaRouter(SENDER);
        require(router.allowanceHolder() == address(holder));
    }

    function testManagementCallsAreUnavailableEvenToDeployer() public {
        bytes[5] memory calls = [
            abi.encodeWithSignature("setPaused(bool)", true),
            abi.encodeWithSignature(
                "setAllowed(address,address,bytes4,bool)",
                address(provider),
                address(provider),
                MockProvider.swap.selector,
                true
            ),
            abi.encodeWithSignature("transferOwnership(address)", SENDER),
            abi.encodeWithSignature("acceptOwnership()"),
            abi.encodeWithSignature("recoverToken(address,address,uint256)", address(sell), SENDER, 1)
        ];
        for (uint256 i; i < calls.length; ++i) {
            (bool ok,) = address(router).call(calls[i]);
            require(!ok);
        }
        require(_run(100, 50, 90, 60) == 60);
    }

    function testNativeRefundRejectionRollsBackSwap() public {
        RejectNative sender = new RejectNative();
        address native = router.NATIVE();
        vm.deal(address(sender), 1 ether);
        bytes memory execution = _executionTo(
            address(sender), native, 1 ether, 1, 1 ether, _data(0, 1, address(router), 0.4 ether), block.timestamp + 60
        );
        vm.prank(address(sender));
        vm.expectRevert(MetaRouter.NativeRefundFailed.selector);
        holder.exec{value: 1 ether}(address(router), native, 1 ether, payable(address(router)), execution);
        require(address(sender).balance == 1 ether);
        require(buy.balanceOf(address(sender)) == 0);
        require(address(provider).balance == 0);
    }

    function testMinimumCheckedAfterNativeRefundCallback() public {
        SwapCallback sender = new SwapCallback();
        sender.configure(address(buy), abi.encodeCall(MockToken.transfer, (SENDER, 1)));
        address native = router.NATIVE();
        vm.deal(address(sender), 1 ether);
        bytes memory execution = _executionTo(
            address(sender), native, 1 ether, 1, 1 ether, _data(0, 1, address(router), 0.4 ether), block.timestamp + 60
        );
        vm.prank(address(sender));
        vm.expectRevert(MetaRouter.InsufficientOutput.selector);
        holder.exec{value: 1 ether}(address(router), native, 1 ether, payable(address(router)), execution);
        require(address(sender).balance == 1 ether);
        require(buy.balanceOf(address(sender)) == 0);
        require(buy.balanceOf(SENDER) == 0);
        require(address(provider).balance == 0);
    }

    function testNativeRefundCallbackCannotReenterSwap() public {
        SwapCallback sender = new SwapCallback();
        bytes memory inner =
            _execution(address(sell), 100, 1, 0, _data(100, 1, address(router), 0), block.timestamp + 60);
        sender.configure(
            address(holder),
            abi.encodeCall(
                MockAllowanceHolder.exec, (address(router), address(sell), 100, payable(address(router)), inner)
            )
        );
        address native = router.NATIVE();
        vm.deal(address(sender), 1 ether);
        bytes memory execution = _executionTo(
            address(sender), native, 1 ether, 1, 1 ether, _data(0, 1, address(router), 0.4 ether), block.timestamp + 60
        );
        vm.prank(address(sender));
        holder.exec{value: 1 ether}(address(router), native, 1 ether, payable(address(router)), execution);
        require(!sender.succeeded());
        require(keccak256(sender.result()) == keccak256(abi.encodePacked(MetaRouter.Reentrant.selector)));
        require(address(sender).balance == 0.4 ether);
        require(buy.balanceOf(address(sender)) == 1);
    }

    function testProviderWithBalanceOfNeedsNoRegistration() public {
        MockBalanceReportingProvider fresh = new MockBalanceReportingProvider();
        bytes memory execution = abi.encodeCall(
            MetaRouter.execute,
            (
                address(sell),
                address(buy),
                SENDER,
                100,
                50,
                block.timestamp + 60,
                address(fresh),
                address(fresh),
                0,
                _data(100, 60, address(router), 0)
            )
        );
        vm.prank(SENDER);
        holder.exec(address(router), address(sell), 100, payable(address(router)), execution);
        require(buy.balanceOf(SENDER) == 60);
        require(sell.balanceOf(SENDER) == 1000 ether - 100);
        require(sell.allowance(address(router), address(fresh)) == 0);
    }

    function testExistingSenderOutputCannotMeetMinimum() public {
        buy.mint(SENDER, 10000);
        vm.expectRevert(MetaRouter.InsufficientOutput.selector);
        _run(100, 50, 100, 1);
        require(buy.balanceOf(SENDER) == 10000);
    }

    function testMissingForwardedSenderRejected() public {
        bytes memory execution =
            _execution(address(sell), 100, 1, 0, _data(100, 10, address(router), 0), block.timestamp + 60);
        vm.prank(address(holder));
        (bool ok, bytes memory reason) = address(router).call(execution);
        // The low-level call is already checked for failure; the revert payload is ABI-encoded by MetaRouter.
        // forge-lint: disable-next-line(unsafe-typecast)
        require(!ok && bytes4(reason) == MetaRouter.InvalidInput.selector);
    }

    function testHolderAllowanceCannotBeUsedAfterExec() public {
        _run(100, 1, 100, 1);
        vm.prank(address(router));
        vm.expectRevert();
        holder.transferFrom(address(sell), SENDER, address(router), 1);
    }

    function testExpired() public {
        vm.warp(100);
        bytes memory execution = _execution(address(sell), 100, 1, 0, _data(100, 10, address(router), 0), 99);
        vm.prank(SENDER);
        vm.expectRevert(MetaRouter.Expired.selector);
        holder.exec(address(router), address(sell), 100, payable(address(router)), execution);
    }

    function testFalseApprovalRejected() public {
        sell.setFailures(true, false);
        vm.expectRevert(MetaRouter.TokenCallFailed.selector);
        _run(100, 1, 100, 50);
    }

    function testFalseOutputTransferRejected() public {
        buy.setFailures(false, true);
        vm.expectRevert(MetaRouter.TokenCallFailed.selector);
        _run(100, 1, 100, 50);
    }

    function testReentrantProviderRejected() public {
        bytes memory inner = _execution(address(sell), 1, 1, 0, _data(1, 1, address(router), 0), block.timestamp + 60);
        bytes memory outer = _execution(
            address(sell),
            100,
            1,
            0,
            abi.encodeCall(MockProvider.reenter, (holder, router, inner)),
            block.timestamp + 60
        );
        vm.prank(SENDER);
        vm.expectRevert(MetaRouter.Reentrant.selector);
        holder.exec(address(router), address(sell), 100, payable(address(router)), outer);
    }

    function _nested(uint256 spend, uint256 output, uint256 minimum) private returns (MockSettler settler) {
        return _nestedTo(SENDER, spend, output, minimum);
    }

    function _nestedTo(address receiver, uint256 spend, uint256 output, uint256 minimum)
        private
        returns (MockSettler settler)
    {
        settler = new MockSettler(holder);
        bytes memory settlement =
            abi.encodeCall(MockSettler.swap, (address(sell), address(buy), spend, output, address(router)));
        bytes memory inner = abi.encodeCall(
            MockAllowanceHolder.exec, (address(settler), address(sell), 100, payable(address(settler)), settlement)
        );
        bytes memory outer = abi.encodeCall(
            MetaRouter.execute,
            (
                address(sell),
                address(buy),
                receiver,
                100,
                minimum,
                block.timestamp + 60,
                address(holder),
                address(holder),
                0,
                inner
            )
        );
        vm.prank(SENDER);
        holder.exec(address(router), address(sell), 100, payable(address(router)), outer);
    }

    function testNestedHolderPreservesHistoryAndClearsAllowances() public {
        sell.mint(address(router), 2000);
        buy.mint(address(router), 3000);
        MockSettler settler = _nested(80, 60, 50);
        require(sell.balanceOf(SENDER) == 1000 ether - 80);
        require(buy.balanceOf(SENDER) == 60);
        require(sell.balanceOf(address(router)) == 2000);
        require(buy.balanceOf(address(router)) == 3000);
        require(sell.allowance(address(router), address(holder)) == 0);
        require(holder.temporaryAllowance(keccak256(abi.encode(address(router), SENDER, address(sell)))) == 0);
        require(holder.temporaryAllowance(keccak256(abi.encode(address(settler), address(router), address(sell)))) == 0);
    }

    function testNestedHolderCannotOverspendHistoricalFunds() public {
        sell.mint(address(router), 2000);
        // External self-call allows expecting the entire helper, including setup calls, to revert.
        vm.expectRevert();
        this.nestedFailure(101, 60, 50);
        require(sell.balanceOf(address(router)) == 2000);
        require(sell.balanceOf(SENDER) == 1000 ether);
    }

    function testNestedHolderMinimumEnforced() public {
        vm.expectRevert(MetaRouter.InsufficientOutput.selector);
        this.nestedFailure(100, 49, 50);
        require(sell.balanceOf(SENDER) == 1000 ether);
    }

    function nestedFailure(uint256 spend, uint256 output, uint256 minimum) external {
        _nested(spend, output, minimum);
    }

    function testInvalidTargetsAndSpendersRejected() public {
        bytes memory data = _data(100, 10, address(router), 0);
        _rejectedRoute(address(router), address(provider), data, MetaRouter.InvalidInput.selector);
        _rejectedRoute(address(provider), address(router), data, MetaRouter.InvalidInput.selector);
        _rejectedRoute(address(sell), address(provider), data, MetaRouter.InvalidInput.selector);
        _rejectedRoute(address(buy), address(provider), data, MetaRouter.InvalidInput.selector);
        _rejectedRoute(address(0), address(provider), data, MetaRouter.InvalidInput.selector);
        _rejectedRoute(SENDER, address(provider), data, MetaRouter.InvalidInput.selector);
        _rejectedRoute(address(provider), address(0), data, MetaRouter.InvalidInput.selector);
        _rejectedRoute(address(provider), SENDER, data, MetaRouter.InvalidInput.selector);
        _rejectedRoute(address(provider), address(provider), hex"123456", MetaRouter.InvalidInput.selector);
    }

    function _rejectedRoute(address target, address spender, bytes memory data, bytes4 reason) private {
        bytes memory execution = abi.encodeCall(
            MetaRouter.execute,
            (address(sell), address(buy), SENDER, 100, 1, block.timestamp + 60, spender, target, 0, data)
        );
        vm.prank(SENDER);
        vm.expectRevert(reason);
        holder.exec(address(router), address(sell), 100, payable(address(router)), execution);
    }

    function testUnrelatedStoredTokenIsOutsideSwapGuarantees() public {
        MockToken stored = new MockToken();
        stored.mint(address(router), 1000);
        SwapCallback sender = new SwapCallback();
        sender.configure(address(buy), abi.encodeCall(MockToken.mint, (address(sender), 1)));
        vm.deal(address(sender), 1 ether);
        address native = router.NATIVE();
        // Unrelated idle assets are intentionally unprotected. The native input is
        // refunded and the sender receives the required buy-token increment each time.
        for (uint256 i; i < 2; ++i) {
            bytes memory data = i == 0
                ? abi.encodeCall(MockToken.transfer, (address(sender), 1000))
                : abi.encodeCall(MockToken.approve, (address(sender), 1000));
            bytes memory execution = abi.encodeCall(
                MetaRouter.execute,
                (
                    native,
                    address(buy),
                    address(sender),
                    1 ether,
                    1,
                    block.timestamp + 60,
                    address(provider),
                    address(stored),
                    0,
                    data
                )
            );
            vm.prank(address(sender));
            holder.exec{value: 1 ether}(address(router), native, 1 ether, payable(address(router)), execution);
        }
        require(stored.balanceOf(address(router)) == 0);
        require(stored.balanceOf(address(sender)) == 1000);
        require(stored.allowance(address(router), address(sender)) == 1000);
        require(buy.balanceOf(address(sender)) == 2);
        require(address(sender).balance == 1 ether);
    }
}

contract RejectNative {
    receive() external payable {
        revert();
    }
}

/// @dev Old-style ERC20 fixture: approve/transfer return nothing; Holder's transferFrom path returns bool.
contract NoReturnToken {
    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;

    function mint(address to, uint256 amount) external {
        balanceOf[to] += amount;
    }

    function approve(address spender, uint256 amount) external {
        allowance[msg.sender][spender] = amount;
    }

    function transfer(address to, uint256 amount) external {
        balanceOf[msg.sender] -= amount;
        balanceOf[to] += amount;
    }

    function transferFrom(address from, address to, uint256 amount) external returns (bool) {
        allowance[from][msg.sender] -= amount;
        balanceOf[from] -= amount;
        balanceOf[to] += amount;
        return true;
    }
}

contract SwapCallback {
    address private target;
    bytes private data;
    bool public succeeded;
    bytes public result;

    function configure(address target_, bytes calldata data_) external {
        target = target_;
        data = data_;
    }

    receive() external payable {
        (succeeded, result) = target.call(data);
    }
}
