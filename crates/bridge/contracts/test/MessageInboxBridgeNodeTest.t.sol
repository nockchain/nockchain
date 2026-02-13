// SPDX-License-Identifier: MIT
pragma solidity ^0.8.19;

import {Test} from "forge-std/Test.sol";
import {MessageInbox} from "../MessageInbox.sol";
import {Nock} from "../Nock.sol";

/// @notice Tests for bridge node validation and deduplication
contract MessageInboxBridgeNodeTest is Test {
    MessageInbox internal inbox;
    Nock internal nock;

    uint256[5] internal bridgeNodePrivateKeys;
    address[5] internal bridgeNodeAddresses;

    function setUp() public {
        for (uint256 i = 0; i < 5; i++) {
            uint256 pk = uint256(keccak256(abi.encodePacked("bridge-node", i)));
            bridgeNodePrivateKeys[i] = pk;
            bridgeNodeAddresses[i] = vm.addr(pk);
        }
    }

    function test_initialize_rejects_duplicate_bridge_nodes() public {
        inbox = new MessageInbox();
        nock = new Nock("Nock", "NOCK", address(inbox));

        // Create nodes array with duplicate at index 2 and 4
        address[5] memory nodes;
        nodes[0] = bridgeNodeAddresses[0];
        nodes[1] = bridgeNodeAddresses[1];
        nodes[2] = bridgeNodeAddresses[2];
        nodes[3] = bridgeNodeAddresses[3];
        nodes[4] = bridgeNodeAddresses[2]; // Duplicate!

        vm.expectRevert("Duplicate bridge node address");
        inbox.initialize(nodes, address(nock));
    }

    function test_initialize_rejects_zero_address_bridge_node() public {
        inbox = new MessageInbox();
        nock = new Nock("Nock", "NOCK", address(inbox));

        address[5] memory nodes;
        nodes[0] = bridgeNodeAddresses[0];
        nodes[1] = bridgeNodeAddresses[1];
        nodes[2] = address(0); // Zero address!
        nodes[3] = bridgeNodeAddresses[3];
        nodes[4] = bridgeNodeAddresses[4];

        vm.expectRevert("Bridge node cannot be zero address");
        inbox.initialize(nodes, address(nock));
    }

    function test_initialize_accepts_unique_bridge_nodes() public {
        inbox = new MessageInbox();
        nock = new Nock("Nock", "NOCK", address(inbox));

        address[5] memory nodes;
        for (uint256 i = 0; i < 5; i++) {
            nodes[i] = bridgeNodeAddresses[i];
        }

        inbox.initialize(nodes, address(nock));

        // Verify all nodes were set correctly
        for (uint256 i = 0; i < 5; i++) {
            assertEq(inbox.bridgeNodes(i), bridgeNodeAddresses[i]);
        }
    }

    function test_updateBridgeNode_rejects_duplicate() public {
        inbox = new MessageInbox();
        nock = new Nock("Nock", "NOCK", address(inbox));

        address[5] memory nodes;
        for (uint256 i = 0; i < 5; i++) {
            nodes[i] = bridgeNodeAddresses[i];
        }
        inbox.initialize(nodes, address(nock));

        // Try to update node 0 to be the same as node 1
        vm.expectRevert("Duplicate bridge node address");
        inbox.updateBridgeNode(0, bridgeNodeAddresses[1]);
    }

    function test_updateBridgeNode_allows_same_address_in_same_slot() public {
        inbox = new MessageInbox();
        nock = new Nock("Nock", "NOCK", address(inbox));

        address[5] memory nodes;
        for (uint256 i = 0; i < 5; i++) {
            nodes[i] = bridgeNodeAddresses[i];
        }
        inbox.initialize(nodes, address(nock));

        // Updating a slot to its current value should work (no-op)
        inbox.updateBridgeNode(0, bridgeNodeAddresses[0]);
        assertEq(inbox.bridgeNodes(0), bridgeNodeAddresses[0]);
    }

    function test_updateBridgeNode_allows_new_unique_address() public {
        inbox = new MessageInbox();
        nock = new Nock("Nock", "NOCK", address(inbox));

        address[5] memory nodes;
        for (uint256 i = 0; i < 5; i++) {
            nodes[i] = bridgeNodeAddresses[i];
        }
        inbox.initialize(nodes, address(nock));

        // Create a new unique address
        address newNode = vm.addr(uint256(keccak256(abi.encodePacked("new-bridge-node"))));

        inbox.updateBridgeNode(0, newNode);
        assertEq(inbox.bridgeNodes(0), newNode);
    }

    function test_updateBridgeNode_rejects_zero_address() public {
        inbox = new MessageInbox();
        nock = new Nock("Nock", "NOCK", address(inbox));

        address[5] memory nodes;
        for (uint256 i = 0; i < 5; i++) {
            nodes[i] = bridgeNodeAddresses[i];
        }
        inbox.initialize(nodes, address(nock));

        vm.expectRevert("Invalid bridge node address");
        inbox.updateBridgeNode(0, address(0));
    }

    function test_updateBridgeNode_only_owner() public {
        inbox = new MessageInbox();
        nock = new Nock("Nock", "NOCK", address(inbox));

        address[5] memory nodes;
        for (uint256 i = 0; i < 5; i++) {
            nodes[i] = bridgeNodeAddresses[i];
        }
        inbox.initialize(nodes, address(nock));

        address newNode = vm.addr(uint256(keccak256(abi.encodePacked("new-bridge-node"))));
        address notOwner = vm.addr(uint256(keccak256(abi.encodePacked("not-owner"))));

        vm.prank(notOwner);
        vm.expectRevert();
        inbox.updateBridgeNode(0, newNode);
    }
}
