// SPDX-License-Identifier: MIT
pragma solidity ^0.8.19;

import {Ownable} from "@openzeppelin/contracts/access/Ownable.sol";
import {stdJson} from "forge-std/StdJson.sol";
import {Nock} from "../Nock.sol";
import {BridgeTestBase} from "./BridgeTestBase.t.sol";

contract NockWithdrawalTest is BridgeTestBase {
    using stdJson for string;

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

    function testBurnAcceptsCanonicalWithdrawalWireV1Vector() public {
        string memory fixture = vm.readFile("../test-fixtures/withdrawal_wire_v1_vectors.json");
        address burner = fixture.readAddress(".valid_vectors[0].burner_address");
        uint256 amount = vm.parseUint(fixture.readString(".valid_vectors[0].amount_base_units"));
        bytes32 commitment = fixture.readBytes32(".valid_vectors[0].commitment");
        bytes memory data = fixture.readBytes(".valid_vectors[0].calldata");

        assertEq(data.length, 116);
        mintFromInbox(burner, amount);

        vm.expectEmit(true, true, true, true, address(nock));
        emit Nock.BurnForWithdrawal(burner, amount, commitment);

        vm.prank(burner);
        (bool ok, bytes memory returnData) = address(nock).call(data);
        assertTrue(ok, string(returnData));
        assertEq(nock.balanceOf(burner), 0);
    }

    function testBurnRevertsAtomicallyWhenWithdrawalsDisabled() public {
        address burner = makeAddr("disabled-burner");
        uint256 amount = nockAmount(1);
        mintFromInbox(burner, amount);
        inbox.setWithdrawalsEnabled(false);

        vm.prank(burner);
        vm.expectRevert("Withdrawals are disabled");
        nock.burn(amount, keccak256("disabled-lock"));

        assertEq(nock.balanceOf(burner), amount);
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
