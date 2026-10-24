// SPDX-License-Identifier: MIT
pragma solidity ^0.8.19;

import "forge-std/Script.sol";
import "forge-std/StdJson.sol";
import "forge-std/console2.sol";

import {MessageInbox} from "../MessageInbox.sol";

contract SetBridgeNodes is Script {
    using stdJson for string;

    uint256 private constant NODE_COUNT = 5;

    function run() external {
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
        address inboxAddress = deploymentsFile.readAddress(
            "$.messageInboxProxy"
        );
        MessageInbox inbox = MessageInbox(inboxAddress);

        address[] memory desiredNodes = new address[](NODE_COUNT);
        for (uint256 i = 0; i < NODE_COUNT; i++) {
            string memory envName = string.concat(
                "BRIDGE_NODE_ADDR_",
                vm.toString(i)
            );
            desiredNodes[i] = vm.envAddress(envName);
        }

        uint256 ownerKey = vm.envUint("INBOX_PRIVATE_KEY");
        address owner = vm.addr(ownerKey);
        console2.log(
            string.concat("Updating bridge nodes as ", vm.toString(owner))
        );

        vm.startBroadcast(ownerKey);
        for (uint256 i = 0; i < NODE_COUNT; i++) {
            address current = inbox.bridgeNodes(i);
            if (current != desiredNodes[i]) {
                console2.log(
                    string.concat(
                        "Updating node ",
                        vm.toString(i),
                        " from ",
                        vm.toString(current),
                        " to ",
                        vm.toString(desiredNodes[i])
                    )
                );
                inbox.updateBridgeNode(i, desiredNodes[i]);
            } else {
                console2.log(
                    string.concat(
                        "Node ",
                        vm.toString(i),
                        " already set to ",
                        vm.toString(current)
                    )
                );
            }
        }
        vm.stopBroadcast();

        console2.log("Bridge node update complete");
    }
}
