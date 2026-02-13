// SPDX-License-Identifier: MIT
pragma solidity ^0.8.19;

import {Ownable} from "@openzeppelin/contracts/access/Ownable.sol";
import {Nock} from "../Nock.sol";
import {BridgeTestBase} from "./BridgeTestBase.t.sol";

contract NockWithdrawalTest is BridgeTestBase {
    function setUp() public override {
        super.setUp();
    }

    function testBurnEmitsEventAndNotifiesInbox() public {
        address burner = makeAddr("burner");
        uint256 amount = nockAmount(25);
        bytes32 lockRoot = keccak256("lock-root");

        mintFromInbox(burner, amount);

        vm.expectEmit(true, true, true, true, address(nock));
        emit Nock.BurnForWithdrawal(burner, amount, lockRoot);

        vm.prank(burner);
        nock.burn(amount, lockRoot);

        assertEq(nock.balanceOf(burner), 0);
    }

    function testBurnRequiresPositiveAmount() public {
        address burner = makeAddr("burner");
        mintFromInbox(burner, nockAmount(1));

        vm.prank(burner);
        vm.expectRevert("Amount must be positive");
        nock.burn(0, keccak256("lock"));
    }

    function testBurnRequiresSufficientBalance() public {
        address burner = makeAddr("burner");
        vm.prank(burner);
        vm.expectRevert("Insufficient balance");
        nock.burn(nockAmount(1), keccak256("lock"));
    }

    function testMintOnlyInboxCanCall() public {
        vm.expectRevert("Only inbox can mint");
        nock.mint(makeAddr("recipient"), nockAmount(1));
    }

    function testMintRequiresPositiveAmount() public {
        vm.prank(address(inbox));
        vm.expectRevert("Amount must be positive");
        nock.mint(makeAddr("recipient"), 0);
    }

    function testUpdateInboxOnlyOwner() public {
        address newInbox = makeAddr("new-inbox");
        vm.expectEmit(true, true, false, true, address(nock));
        emit Nock.InboxUpdated(address(inbox), newInbox);
        nock.updateInbox(newInbox);
        assertEq(nock.inbox(), newInbox);
    }

    function testUpdateInboxRejectsZeroAddress() public {
        vm.expectRevert("Invalid inbox address");
        nock.updateInbox(address(0));
    }

    function testUpdateInboxRequiresOwner() public {
        address attacker = makeAddr("attacker");
        vm.prank(attacker);
        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableUnauthorizedAccount.selector, attacker));
        nock.updateInbox(makeAddr("new"));
    }

    function testDecimalsReturns16() public view {
        assertEq(nock.decimals(), 16);
    }
}
