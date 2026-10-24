// SPDX-License-Identifier: MIT
pragma solidity ^0.8.19;

import {ERC1967Proxy} from "@openzeppelin/contracts/proxy/ERC1967/ERC1967Proxy.sol";
import {OwnableUpgradeable} from "@openzeppelin/contracts-upgradeable/access/OwnableUpgradeable.sol";
import {MessageInbox} from "../MessageInbox.sol";
import {Nock} from "../Nock.sol";

import {BridgeTestBase} from "./BridgeTestBase.t.sol";

contract MessageInboxUpgradeTest is BridgeTestBase {
    MessageInbox internal implementation;

    function setUp() public override {
        _initBridgeNodes();

        implementation = new MessageInbox();
        nock = new Nock("Nock", "NOCK", address(implementation));

        address[5] memory nodes = bridgeNodesArray();
        bytes memory initCalldata = abi.encodeWithSelector(
            MessageInbox.initialize.selector,
            nodes,
            address(nock)
        );

        ERC1967Proxy proxy = new ERC1967Proxy(
            address(implementation),
            initCalldata
        );

        inbox = MessageInbox(address(proxy));
        nock.updateInbox(address(inbox));
    }

    function testOwnerCanUpgrade() public {
        MessageInboxV2 newImpl = new MessageInboxV2();
        inbox.upgradeTo(address(newImpl));

        assertEq(MessageInboxV2(address(inbox)).version(), 2);
    }

    function testNonOwnerCannotUpgrade() public {
        MessageInboxV2 newImpl = new MessageInboxV2();
        address attacker = makeAddr("attacker");

        vm.prank(attacker);
        vm.expectRevert(abi.encodeWithSelector(OwnableUpgradeable.OwnableUnauthorizedAccount.selector, attacker));
        inbox.upgradeTo(address(newImpl));
    }
}

contract MessageInboxV2 is MessageInbox {
    function version() external pure returns (uint8) {
        return 2;
    }
}
