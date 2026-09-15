// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

/// @notice ERC20 operations used by the router and its AllowanceHolder integration.
interface IERC20 {
    function balanceOf(address account) external view returns (uint256);
    function approve(address spender, uint256 amount) external returns (bool);
    function transfer(address recipient, uint256 amount) external returns (bool);
    function transferFrom(address sender, address recipient, uint256 amount) external returns (bool);
}
