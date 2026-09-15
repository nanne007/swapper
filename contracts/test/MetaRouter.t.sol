// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {MetaRouter} from "../src/MetaRouter.sol";

interface Vm {
    function deal(address account, uint256 amount) external;
    function prank(address sender) external;
    function expectRevert(bytes4 selector) external;
    function expectRevert() external;
    function warp(uint256 timestamp) external;
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
        (bool isToken, bytes memory balance) =
            target.staticcall(abi.encodeWithSignature("balanceOf(address)", address(0xdead)));
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
    function recoverDuringSwap(MetaRouter router, address token) external {
        router.recoverToken(token, payable(address(this)), 1);
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

/// @dev A non-token route entry may expose balanceOf; allowlist review decides eligibility.
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
    address private constant TAKER = address(0xBEEF);
    MetaRouter private router;
    MockAllowanceHolder private holder;
    MockToken private sell;
    MockToken private buy;
    MockProvider private provider;

    function setUp() public {
        holder = new MockAllowanceHolder();
        router = new MetaRouter(address(this), address(holder));
        sell = new MockToken();
        buy = new MockToken();
        provider = new MockProvider();
        router.setAllowed(address(provider), address(provider), MockProvider.swap.selector, true);
        sell.mint(TAKER, 1000 ether);
        vm.prank(TAKER);
        sell.approve(address(holder), type(uint256).max);
        vm.deal(TAKER, 1000 ether);
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
        return abi.encodeCall(
            MetaRouter.execute,
            (sellToken, address(buy), amount, min, address(provider), address(provider), value, data, deadline)
        );
    }

    function _run(uint256 amount, uint256 minimum, uint256 spend, uint256 output) private returns (uint256) {
        bytes memory execution = _execution(
            address(sell), amount, minimum, 0, _data(spend, output, address(router), 0), block.timestamp + 60
        );
        vm.prank(TAKER);
        bytes memory result = holder.exec(address(router), address(sell), amount, payable(address(router)), execution);
        // expectRevert consumes the revert and returns empty bytes to this helper.
        return result.length == 0 ? 0 : abi.decode(result, (uint256));
    }

    function testSuccessfulSwapAndAllowanceCleanup() public {
        require(_run(100, 80, 90, 85) == 85);
        require(sell.balanceOf(TAKER) == 1000 ether - 90);
        require(buy.balanceOf(TAKER) == 85);
        require(sell.allowance(address(router), address(provider)) == 0);
        require(holder.temporaryAllowance(keccak256(abi.encode(address(router), TAKER, address(sell)))) == 0);
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
        _run(amount, 1, spend, 17);
        require(sell.balanceOf(address(router)) == historicSell);
        require(buy.balanceOf(address(router)) == historicBuy);
        require(address(router).balance == historicNative);
        require(sell.balanceOf(TAKER) == 1000 ether - spend);
        require(buy.balanceOf(TAKER) == 17);
        require(sell.allowance(address(router), address(provider)) == 0);
    }

    function testFuzzMinimumOutput(uint64 output, uint64 rawMinimum) public {
        uint256 minimum = uint256(rawMinimum) + 1;
        if (output < minimum) vm.expectRevert(MetaRouter.InsufficientOutput.selector);
        _run(100, minimum, 100, output);
        if (output < minimum) {
            require(sell.balanceOf(TAKER) == 1000 ether);
            require(buy.balanceOf(TAKER) == 0);
        }
    }

    function testDirectProviderRecipientSupported() public {
        bytes memory execution = _execution(address(sell), 100, 50, 0, _data(100, 60, TAKER, 0), block.timestamp + 60);
        vm.prank(TAKER);
        holder.exec(address(router), address(sell), 100, payable(address(router)), execution);
        require(buy.balanceOf(TAKER) == 60);
    }

    function testNativeRefundPreservesHistory() public {
        address native = router.NATIVE();
        vm.deal(address(router), 7 ether);
        bytes memory execution = _execution(
            router.NATIVE(), 1 ether, 50, 1 ether, _data(0, 60, address(router), 0.4 ether), block.timestamp + 60
        );
        vm.prank(TAKER);
        holder.exec{value: 1 ether}(address(router), native, 1 ether, payable(address(router)), execution);
        require(address(router).balance == 7 ether);
        require(TAKER.balance == 999.4 ether);
        require(buy.balanceOf(TAKER) == 60);
    }

    function testWrongNativeValue() public {
        address native = router.NATIVE();
        bytes memory execution =
            _execution(router.NATIVE(), 1 ether, 1, 1 ether, _data(0, 2, address(router), 0), block.timestamp + 60);
        vm.prank(TAKER);
        vm.expectRevert(MetaRouter.InvalidInput.selector);
        holder.exec{value: 0.5 ether}(address(router), native, 1 ether, payable(address(router)), execution);
    }

    function testFuzzNativeRefund(uint64 rawAmount, uint64 rawRefund, uint96 history) public {
        address native = router.NATIVE();
        uint256 amount = uint256(rawAmount) + 1;
        uint256 refund = uint256(rawRefund) % (amount + 1);
        vm.deal(address(router), history);
        bytes memory execution =
            _execution(native, amount, 1, amount, _data(0, 1, address(router), refund), block.timestamp + 60);
        vm.prank(TAKER);
        holder.exec{value: amount}(address(router), native, amount, payable(address(router)), execution);
        require(address(router).balance == history);
        require(TAKER.balance == 1000 ether - amount + refund);
        require(buy.balanceOf(TAKER) == 1);
    }

    function testERC20RejectsValue() public {
        bytes memory execution =
            _execution(address(sell), 1, 1, 1, _data(1, 2, address(router), 0), block.timestamp + 60);
        vm.prank(TAKER);
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
        vm.prank(TAKER);
        (bool ok,) = address(router).call(bytes.concat(execution, bytes20(TAKER)));
        require(!ok);
    }

    function testCannotForgeAnotherOwnerThroughHolder() public {
        address attacker = address(0xBAD);
        bytes memory execution =
            _execution(address(sell), 100, 1, 0, _data(100, 10, address(router), 0), block.timestamp + 60);
        vm.prank(attacker);
        vm.expectRevert(MetaRouter.InvalidInput.selector);
        holder.exec(
            address(router), address(sell), 100, payable(address(router)), bytes.concat(execution, bytes20(TAKER))
        );
        require(sell.balanceOf(TAKER) == 1000 ether);
    }

    function testWrongOperatorCannotUseEphemeralAllowance() public {
        bytes memory execution =
            _execution(address(sell), 100, 1, 0, _data(100, 10, address(router), 0), block.timestamp + 60);
        vm.prank(TAKER);
        vm.expectRevert();
        holder.exec(address(provider), address(sell), 100, payable(address(router)), execution);
    }

    function testUnlistedTupleRejected() public {
        router.setAllowed(address(provider), address(provider), MockProvider.swap.selector, false);
        vm.expectRevert(MetaRouter.RouteNotAllowed.selector);
        _run(100, 1, 100, 50);
    }

    function testNewTargetRequiresRegistrationAndCanBeRevoked() public {
        MockProvider fresh = new MockProvider();
        bytes32 key = keccak256(abi.encode(address(fresh), address(fresh), MockProvider.swap.selector));
        bytes memory execution = abi.encodeCall(
            MetaRouter.execute,
            (
                address(sell),
                address(buy),
                100,
                50,
                address(fresh),
                address(fresh),
                0,
                _data(100, 60, address(router), 0),
                block.timestamp + 60
            )
        );
        require(!router.allowed(key));
        vm.prank(TAKER);
        vm.expectRevert(MetaRouter.RouteNotAllowed.selector);
        holder.exec(address(router), address(sell), 100, payable(address(router)), execution);
        router.setAllowed(address(fresh), address(fresh), MockProvider.swap.selector, true);
        require(router.allowed(key));
        vm.prank(TAKER);
        holder.exec(address(router), address(sell), 100, payable(address(router)), execution);
        require(buy.balanceOf(TAKER) == 60);
        require(sell.allowance(address(router), address(fresh)) == 0);
        router.setAllowed(address(fresh), address(fresh), MockProvider.swap.selector, false);
        require(!router.allowed(key));
        vm.prank(TAKER);
        vm.expectRevert(MetaRouter.RouteNotAllowed.selector);
        holder.exec(address(router), address(sell), 100, payable(address(router)), execution);
        require(buy.balanceOf(TAKER) == 60);
    }

    function testAllowlistedProviderWithBalanceOfCanExecute() public {
        MockBalanceReportingProvider reviewed = new MockBalanceReportingProvider();
        bytes memory execution = abi.encodeCall(
            MetaRouter.execute,
            (
                address(sell),
                address(buy),
                100,
                50,
                address(reviewed),
                address(reviewed),
                0,
                _data(100, 60, address(router), 0),
                block.timestamp + 60
            )
        );
        vm.prank(TAKER);
        vm.expectRevert(MetaRouter.RouteNotAllowed.selector);
        holder.exec(address(router), address(sell), 100, payable(address(router)), execution);
        router.setAllowed(address(reviewed), address(reviewed), MockProvider.swap.selector, true);
        vm.prank(TAKER);
        holder.exec(address(router), address(sell), 100, payable(address(router)), execution);
        require(buy.balanceOf(TAKER) == 60);
        require(sell.balanceOf(TAKER) == 1000 ether - 100);
        require(sell.allowance(address(router), address(reviewed)) == 0);
    }

    function testUnlistedSelectorRejected() public {
        bytes memory execution = _execution(address(sell), 100, 1, 0, hex"12345678", block.timestamp + 60);
        vm.prank(TAKER);
        vm.expectRevert(MetaRouter.RouteNotAllowed.selector);
        holder.exec(address(router), address(sell), 100, payable(address(router)), execution);
    }

    function testUnlistedSpenderRejected() public {
        MockProvider otherSpender = new MockProvider();
        bytes memory execution = abi.encodeCall(
            MetaRouter.execute,
            (
                address(sell),
                address(buy),
                100,
                1,
                address(provider),
                address(otherSpender),
                0,
                _data(100, 10, address(router), 0),
                block.timestamp + 60
            )
        );
        vm.prank(TAKER);
        vm.expectRevert(MetaRouter.RouteNotAllowed.selector);
        holder.exec(address(router), address(sell), 100, payable(address(router)), execution);
    }

    function testExistingTakerOutputCannotMeetMinimum() public {
        buy.mint(TAKER, 10000);
        vm.expectRevert(MetaRouter.InsufficientOutput.selector);
        _run(100, 50, 100, 1);
        require(buy.balanceOf(TAKER) == 10000);
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
        holder.transferFrom(address(sell), TAKER, address(router), 1);
    }

    function testExpired() public {
        vm.warp(100);
        bytes memory execution = _execution(address(sell), 100, 1, 0, _data(100, 10, address(router), 0), 99);
        vm.prank(TAKER);
        vm.expectRevert(MetaRouter.Expired.selector);
        holder.exec(address(router), address(sell), 100, payable(address(router)), execution);
    }

    function testPauseAndAdminAuthorization() public {
        vm.prank(TAKER);
        vm.expectRevert(MetaRouter.Unauthorized.selector);
        router.setPaused(true);
        vm.prank(TAKER);
        vm.expectRevert(MetaRouter.Unauthorized.selector);
        router.setAllowed(address(provider), address(provider), MockProvider.swap.selector, true);
        router.setPaused(true);
        vm.expectRevert(MetaRouter.Paused.selector);
        _run(100, 1, 100, 50);
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
        router.setAllowed(address(provider), address(provider), MockProvider.reenter.selector, true);
        bytes memory inner = _execution(address(sell), 1, 1, 0, _data(1, 1, address(router), 0), block.timestamp + 60);
        bytes memory outer = _execution(
            address(sell),
            100,
            1,
            0,
            abi.encodeCall(MockProvider.reenter, (holder, router, inner)),
            block.timestamp + 60
        );
        vm.prank(TAKER);
        vm.expectRevert(MetaRouter.Reentrant.selector);
        holder.exec(address(router), address(sell), 100, payable(address(router)), outer);
    }

    function _nested(uint256 spend, uint256 output, uint256 minimum) private returns (MockSettler settler) {
        settler = new MockSettler(holder);
        router.setAllowed(address(holder), address(holder), MockAllowanceHolder.exec.selector, true);
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
                100,
                minimum,
                address(holder),
                address(holder),
                0,
                inner,
                block.timestamp + 60
            )
        );
        vm.prank(TAKER);
        holder.exec(address(router), address(sell), 100, payable(address(router)), outer);
    }

    function testNestedHolderPreservesHistoryAndClearsAllowances() public {
        sell.mint(address(router), 2000);
        buy.mint(address(router), 3000);
        MockSettler settler = _nested(80, 60, 50);
        require(sell.balanceOf(TAKER) == 1000 ether - 80);
        require(buy.balanceOf(TAKER) == 60);
        require(sell.balanceOf(address(router)) == 2000);
        require(buy.balanceOf(address(router)) == 3000);
        require(sell.allowance(address(router), address(holder)) == 0);
        require(holder.temporaryAllowance(keccak256(abi.encode(address(router), TAKER, address(sell)))) == 0);
        require(holder.temporaryAllowance(keccak256(abi.encode(address(settler), address(router), address(sell)))) == 0);
    }

    function testNestedHolderCannotOverspendHistoricalFunds() public {
        sell.mint(address(router), 2000);
        // External self-call allows expecting the entire helper, including setup calls, to revert.
        vm.expectRevert();
        this.nestedFailure(101, 60, 50);
        require(sell.balanceOf(address(router)) == 2000);
        require(sell.balanceOf(TAKER) == 1000 ether);
    }

    function testNestedHolderMinimumEnforced() public {
        vm.expectRevert(MetaRouter.InsufficientOutput.selector);
        this.nestedFailure(100, 49, 50);
        require(sell.balanceOf(TAKER) == 1000 ether);
    }

    function nestedFailure(uint256 spend, uint256 output, uint256 minimum) external {
        _nested(spend, output, minimum);
    }

    function testHolderNonExecSelectorAndMixedTupleForbidden() public {
        _rejectedRoute(
            address(holder),
            address(holder),
            abi.encodePacked(MockAllowanceHolder.transferFrom.selector),
            MetaRouter.RouteNotAllowed.selector
        );
        _rejectedRoute(
            address(holder),
            address(provider),
            abi.encodePacked(MockAllowanceHolder.exec.selector),
            MetaRouter.RouteNotAllowed.selector
        );
        _rejectedRoute(
            address(provider), address(holder), _data(100, 10, address(router), 0), MetaRouter.RouteNotAllowed.selector
        );
    }

    function testAllowlistRejectsInvalidTargetsAndHolderShapes() public {
        vm.expectRevert(MetaRouter.InvalidInput.selector);
        router.setAllowed(address(holder), address(holder), MockAllowanceHolder.transferFrom.selector, true);
        vm.expectRevert(MetaRouter.InvalidInput.selector);
        router.setAllowed(address(holder), address(provider), MockAllowanceHolder.exec.selector, true);
        vm.expectRevert(MetaRouter.InvalidInput.selector);
        router.setAllowed(address(provider), address(holder), MockProvider.swap.selector, true);
        vm.expectRevert(MetaRouter.InvalidInput.selector);
        router.setAllowed(address(router), address(provider), MockProvider.swap.selector, true);
        vm.expectRevert(MetaRouter.InvalidInput.selector);
        router.setAllowed(address(provider), address(router), MockProvider.swap.selector, true);
        vm.expectRevert(MetaRouter.InvalidInput.selector);
        router.setAllowed(address(0), address(provider), MockProvider.swap.selector, true);
        vm.expectRevert(MetaRouter.InvalidInput.selector);
        router.setAllowed(address(provider), TAKER, MockProvider.swap.selector, true);
        // Revocation must remain possible even if an address no longer has code.
        router.setAllowed(address(0), TAKER, MockProvider.swap.selector, false);
    }

    function testInvalidTargetsAndSpendersRejected() public {
        bytes memory data = _data(100, 10, address(router), 0);
        _rejectedRoute(address(router), address(provider), data, MetaRouter.InvalidInput.selector);
        _rejectedRoute(address(provider), address(router), data, MetaRouter.InvalidInput.selector);
        _rejectedRoute(address(sell), address(provider), data, MetaRouter.InvalidInput.selector);
        _rejectedRoute(address(buy), address(provider), data, MetaRouter.InvalidInput.selector);
        _rejectedRoute(address(0), address(provider), data, MetaRouter.InvalidInput.selector);
        _rejectedRoute(TAKER, address(provider), data, MetaRouter.InvalidInput.selector);
        _rejectedRoute(address(provider), address(0), data, MetaRouter.InvalidInput.selector);
        _rejectedRoute(address(provider), TAKER, data, MetaRouter.InvalidInput.selector);
        _rejectedRoute(address(provider), address(provider), hex"123456", MetaRouter.InvalidInput.selector);
    }

    function _rejectedRoute(address target, address spender, bytes memory data, bytes4 reason) private {
        bytes memory execution = abi.encodeCall(
            MetaRouter.execute, (address(sell), address(buy), 100, 1, target, spender, 0, data, block.timestamp + 60)
        );
        vm.prank(TAKER);
        vm.expectRevert(reason);
        holder.exec(address(router), address(sell), 100, payable(address(router)), execution);
    }

    function testRecoverERC20AndNativeWhilePaused() public {
        sell.mint(address(router), 100);
        vm.deal(address(router), 1 ether);
        router.setPaused(true);
        router.recoverToken(address(sell), payable(TAKER), 40);
        router.recoverToken(router.NATIVE(), payable(TAKER), 0.4 ether);
        require(sell.balanceOf(address(router)) == 60);
        require(sell.balanceOf(TAKER) == 1000 ether + 40);
        require(address(router).balance == 0.6 ether);
        require(TAKER.balance == 1000.4 ether);
        require(router.paused());
    }

    function testUnlistedRouteCannotTransferOrApproveUnrelatedStoredToken() public {
        MockToken stored = new MockToken();
        stored.mint(address(router), 1000);
        RecoveryCallback taker = new RecoveryCallback();
        taker.configure(address(buy), abi.encodeCall(MockToken.mint, (address(taker), 1)));
        vm.deal(address(taker), 1 ether);
        address native = router.NATIVE();
        // Minimum output cannot protect third-token balances; unreviewed routes
        // must be rejected by the allowlist before any transfer or approval.
        for (uint256 i; i < 2; ++i) {
            bytes memory data = i == 0
                ? abi.encodeCall(MockToken.transfer, (address(taker), 1000))
                : abi.encodeCall(MockToken.approve, (address(taker), 1000));
            bytes memory execution = abi.encodeCall(
                MetaRouter.execute,
                (native, address(buy), 1 ether, 1, address(stored), address(provider), 0, data, block.timestamp + 60)
            );
            vm.prank(address(taker));
            vm.expectRevert(MetaRouter.RouteNotAllowed.selector);
            holder.exec{value: 1 ether}(address(router), native, 1 ether, payable(address(router)), execution);
        }
        require(stored.balanceOf(address(router)) == 1000);
        require(stored.balanceOf(address(taker)) == 0);
        require(stored.allowance(address(router), address(taker)) == 0);
    }

    function testFuzzRecoverERC20(uint96 balance, uint96 rawAmount) public {
        uint256 amount = uint256(rawAmount) % (uint256(balance) + 1) + 1;
        sell.mint(address(router), uint256(balance) + 1);
        router.recoverToken(address(sell), payable(TAKER), amount);
        require(sell.balanceOf(address(router)) == uint256(balance) + 1 - amount);
        require(sell.balanceOf(TAKER) == 1000 ether + amount);
    }

    function testFuzzRecoverNative(uint96 balance, uint96 rawAmount) public {
        uint256 amount = uint256(rawAmount) % (uint256(balance) + 1) + 1;
        vm.deal(address(router), uint256(balance) + 1);
        router.recoverToken(router.NATIVE(), payable(TAKER), amount);
        require(address(router).balance == uint256(balance) + 1 - amount);
        require(TAKER.balance == 1000 ether + amount);
    }

    function testRecoverRequiresOwner() public {
        sell.mint(address(router), 100);
        vm.deal(address(router), 1 ether);
        address native = router.NATIVE();
        vm.prank(TAKER);
        vm.expectRevert(MetaRouter.Unauthorized.selector);
        router.recoverToken(address(sell), payable(TAKER), 1);
        vm.prank(TAKER);
        vm.expectRevert(MetaRouter.Unauthorized.selector);
        router.recoverToken(native, payable(TAKER), 1);
        require(sell.balanceOf(address(router)) == 100);
        require(address(router).balance == 1 ether);
    }

    function testRecoverRejectsInvalidInputs() public {
        vm.expectRevert(MetaRouter.InvalidInput.selector);
        router.recoverToken(address(sell), payable(address(0)), 1);
        vm.expectRevert(MetaRouter.InvalidInput.selector);
        router.recoverToken(address(sell), payable(address(router)), 1);
        vm.expectRevert(MetaRouter.InvalidInput.selector);
        router.recoverToken(address(sell), payable(TAKER), 0);
        vm.expectRevert(MetaRouter.InvalidInput.selector);
        router.recoverToken(address(0), payable(TAKER), 1);
        vm.expectRevert(MetaRouter.InvalidInput.selector);
        router.recoverToken(TAKER, payable(TAKER), 1);
    }

    function testRecoverFalseTransferAndInsufficientBalanceRevert() public {
        sell.mint(address(router), 10);
        sell.setFailures(false, true);
        vm.expectRevert(MetaRouter.TokenCallFailed.selector);
        router.recoverToken(address(sell), payable(TAKER), 1);
        sell.setFailures(false, false);
        vm.expectRevert(MetaRouter.TokenCallFailed.selector);
        router.recoverToken(address(sell), payable(TAKER), 11);
        address native = router.NATIVE();
        vm.deal(address(router), 10);
        vm.expectRevert(MetaRouter.NativeTransferFailed.selector);
        router.recoverToken(native, payable(TAKER), 11);
        require(sell.balanceOf(address(router)) == 10);
        require(address(router).balance == 10);
    }

    function testRecoverSupportsNoReturnToken() public {
        NoReturnToken token = new NoReturnToken();
        token.mint(address(router), 10);
        router.recoverToken(address(token), payable(TAKER), 4);
        require(token.balanceOf(TAKER) == 4);
        require(token.balanceOf(address(router)) == 6);
    }

    function testRecoverRejectingNativeRecipientRollsBack() public {
        RejectNative recipient = new RejectNative();
        vm.deal(address(router), 10);
        address native = router.NATIVE();
        vm.expectRevert(MetaRouter.NativeTransferFailed.selector);
        router.recoverToken(native, payable(address(recipient)), 4);
        require(address(router).balance == 10);
    }

    function testOwnershipTransferRequiresAcceptanceAndRevokesOldOwner() public {
        bytes32 key = keccak256(abi.encode(address(provider), address(provider), MockProvider.swap.selector));
        router.transferOwnership(TAKER);
        require(router.owner() == address(this));
        require(router.pendingOwner() == TAKER);
        vm.prank(TAKER);
        vm.expectRevert(MetaRouter.Unauthorized.selector);
        router.setAllowed(address(provider), address(provider), MockProvider.swap.selector, false);
        vm.prank(TAKER);
        vm.expectRevert(MetaRouter.Unauthorized.selector);
        router.setPaused(true);
        vm.expectRevert(MetaRouter.Unauthorized.selector);
        router.acceptOwnership();
        vm.prank(TAKER);
        router.acceptOwnership();
        require(router.owner() == TAKER);
        require(router.pendingOwner() == address(0));
        require(router.allowed(key));
        vm.expectRevert(MetaRouter.Unauthorized.selector);
        router.setAllowed(address(provider), address(provider), MockProvider.swap.selector, false);
        vm.prank(TAKER);
        router.setAllowed(address(provider), address(provider), MockProvider.swap.selector, false);
        require(!router.allowed(key));
        vm.expectRevert(MetaRouter.Unauthorized.selector);
        router.setPaused(true);
        vm.expectRevert(MetaRouter.Unauthorized.selector);
        router.recoverToken(address(sell), payable(TAKER), 1);
        vm.expectRevert(MetaRouter.Unauthorized.selector);
        router.transferOwnership(address(this));
        vm.prank(TAKER);
        router.setPaused(true);
        sell.mint(address(router), 10);
        vm.prank(TAKER);
        router.recoverToken(address(sell), payable(TAKER), 10);
        require(sell.balanceOf(address(router)) == 0);
        vm.prank(TAKER);
        vm.expectRevert(MetaRouter.Unauthorized.selector);
        router.acceptOwnership();
    }

    function testOwnerCanReplacePendingNominee() public {
        router.transferOwnership(TAKER);
        router.transferOwnership(address(0xCAFE));
        vm.prank(TAKER);
        vm.expectRevert(MetaRouter.Unauthorized.selector);
        router.acceptOwnership();
        vm.prank(address(0xCAFE));
        router.acceptOwnership();
        require(router.owner() == address(0xCAFE));
    }

    function testOwnershipTransferRejectsInvalidNomineesAndUnauthorizedCaller() public {
        vm.prank(TAKER);
        vm.expectRevert(MetaRouter.Unauthorized.selector);
        router.transferOwnership(TAKER);
        vm.expectRevert(MetaRouter.InvalidInput.selector);
        router.transferOwnership(address(0));
        vm.expectRevert(MetaRouter.InvalidInput.selector);
        router.transferOwnership(address(router));
        vm.expectRevert(MetaRouter.InvalidInput.selector);
        router.transferOwnership(address(holder));
        address native = router.NATIVE();
        vm.expectRevert(MetaRouter.InvalidInput.selector);
        router.transferOwnership(native);
    }

    function testOwnerCannotRecoverDuringSwapCallback() public {
        router.setAllowed(address(provider), address(provider), MockProvider.recoverDuringSwap.selector, true);
        router.transferOwnership(address(provider));
        vm.prank(address(provider));
        router.acceptOwnership();
        sell.mint(address(router), 1000);
        bytes memory execution = _execution(
            address(sell),
            100,
            1,
            0,
            abi.encodeCall(MockProvider.recoverDuringSwap, (router, address(sell))),
            block.timestamp + 60
        );
        vm.prank(TAKER);
        vm.expectRevert(MetaRouter.Reentrant.selector);
        holder.exec(address(router), address(sell), 100, payable(address(router)), execution);
        require(sell.balanceOf(address(router)) == 1000);
        require(sell.balanceOf(TAKER) == 1000 ether);
    }

    function testRecoveryCallbackCannotReenterRecoveryOrSwap() public {
        RecoveryCallback recipient = new RecoveryCallback();
        router.transferOwnership(address(recipient));
        vm.prank(address(recipient));
        router.acceptOwnership();
        vm.deal(address(router), 10);
        address native = router.NATIVE();
        recipient.configure(address(router), abi.encodeCall(MetaRouter.recoverToken, (native, payable(TAKER), 1)));
        vm.prank(address(recipient));
        router.recoverToken(native, payable(address(recipient)), 1);
        require(!recipient.succeeded());
        require(keccak256(recipient.result()) == keccak256(abi.encodePacked(MetaRouter.Reentrant.selector)));

        bytes memory execution =
            _execution(address(sell), 100, 1, 0, _data(100, 10, address(router), 0), block.timestamp + 60);
        recipient.configure(
            address(holder),
            abi.encodeCall(
                MockAllowanceHolder.exec, (address(router), address(sell), 100, payable(address(router)), execution)
            )
        );
        vm.prank(address(recipient));
        router.recoverToken(native, payable(address(recipient)), 1);
        require(!recipient.succeeded());
        require(keccak256(recipient.result()) == keccak256(abi.encodePacked(MetaRouter.Reentrant.selector)));
        require(address(router).balance == 8);
        require(sell.balanceOf(TAKER) == 1000 ether);
    }
}

contract NoReturnToken {
    mapping(address => uint256) public balanceOf;

    function mint(address to, uint256 amount) external {
        balanceOf[to] += amount;
    }

    function transfer(address to, uint256 amount) external {
        balanceOf[msg.sender] -= amount;
        balanceOf[to] += amount;
    }
}

contract RejectNative {
    receive() external payable {
        revert();
    }
}

contract RecoveryCallback {
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
