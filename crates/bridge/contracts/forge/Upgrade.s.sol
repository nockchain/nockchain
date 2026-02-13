// SPDX-License-Identifier: MIT
pragma solidity ^0.8.19;

import "forge-std/Script.sol";
import "forge-std/StdJson.sol";
import "forge-std/console2.sol";

import {MessageInbox} from "../MessageInbox.sol";

/// @notice Upgrades the MessageInbox implementation via UUPS proxy
///         Reads deployment addresses from DEPLOYMENTS_PATH
///         Requires INBOX_PRIVATE_KEY to be set (must be owner)
contract Upgrade is Script {
    using stdJson for string;

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
        address proxy = deploymentsFile.readAddress("$.messageInboxProxy");

        uint256 ownerKey = vm.envUint("INBOX_PRIVATE_KEY");

        vm.startBroadcast(ownerKey);
        MessageInbox newImplementation = new MessageInbox();
        MessageInbox inbox = MessageInbox(proxy);
        inbox.upgradeTo(address(newImplementation));
        vm.stopBroadcast();

        console2.log("Upgraded to:", address(newImplementation));
    }
}

