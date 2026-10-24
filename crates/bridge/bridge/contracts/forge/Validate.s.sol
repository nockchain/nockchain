// SPDX-License-Identifier: MIT
pragma solidity ^0.8.19;

import "forge-std/Script.sol";
import "forge-std/StdJson.sol";
import "forge-std/console2.sol";

import {MessageInbox} from "../MessageInbox.sol";
import {Nock} from "../Nock.sol";

/// @notice Validates a deployed bridge contract configuration
///         Reads deployment addresses from DEPLOYMENTS_PATH (defaults to deployments.json)
contract Validate is Script {
    using stdJson for string;

    function run() external view {
        string memory deploymentsPath = vm.envOr(
            "DEPLOYMENTS_PATH",
            string("deployments.json")
        );

        string memory fullPath = bytes(deploymentsPath).length > 0
            ? deploymentsPath
            : "deployments.json";

        if (bytes(fullPath)[0] != "/") {
            fullPath = string.concat(vm.projectRoot(), "/", fullPath);
        }

        string memory deploymentsFile = vm.readFile(fullPath);
        address implementation = deploymentsFile.readAddress(
            "$.messageInboxImplementation"
        );
        address proxy = deploymentsFile.readAddress("$.messageInboxProxy");
        address nock = deploymentsFile.readAddress("$.nock");

        MessageInbox inbox = MessageInbox(proxy);
        Nock nockToken = Nock(nock);

        bytes32 implSlot = 0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc;
        bytes32 slotValue = vm.load(proxy, implSlot);
        address currentImpl = address(uint160(uint256(slotValue)));
        require(
            currentImpl == implementation,
            "Proxy implementation mismatch"
        );

        require(
            nockToken.inbox() == proxy,
            "Nock token not connected to inbox proxy"
        );

        // Verify all bridge nodes are set and unique
        address[5] memory nodes;
        for (uint256 i = 0; i < 5; i++) {
            nodes[i] = inbox.bridgeNodes(i);
            require(nodes[i] != address(0), "Bridge node not set");
        }

        // Check for duplicates
        for (uint256 i = 0; i < 5; i++) {
            for (uint256 j = i + 1; j < 5; j++) {
                require(
                    nodes[i] != nodes[j],
                    "Duplicate bridge node addresses detected"
                );
            }
        }

        require(inbox.THRESHOLD() == 3, "Invalid threshold");
        require(inbox.withdrawalsEnabled(), "Withdrawals not enabled");
        require(inbox.owner() != address(0), "Owner not set");
    }
}

