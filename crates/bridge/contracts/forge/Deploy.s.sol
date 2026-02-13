// SPDX-License-Identifier: MIT
pragma solidity ^0.8.19;

import "forge-std/Script.sol";
import "forge-std/StdJson.sol";
import "forge-std/console2.sol";
import {ERC1967Proxy} from "openzeppelin-contracts/proxy/ERC1967/ERC1967Proxy.sol";

import {MessageInbox} from "../MessageInbox.sol";
import {Nock} from "../Nock.sol";

/// @notice Deploys MessageInbox behind an ERC1967 proxy and a paired Nock token.
///         Expects the following environment variables to be set before execution:
///         - BRIDGE_NODE_0 .. BRIDGE_NODE_4
///         - NOCK_NAME
///         - NOCK_SYMBOL
///         Optional helpers:
///         - DEPLOY_TARGET_NETWORK (defaults to "tenderly-devnet")
///         - DEPLOYMENTS_PATH (defaults to "deployments.json")
///         - DEPLOYER_ADDRESS (used only for metadata; defaults to zero address)
contract Deploy is Script {
    using stdJson for string;

    function run() public {
        address[5] memory bridgeNodes = _loadBridgeNodes();
        string memory nockName = vm.envString("NOCK_NAME");
        string memory nockSymbol = vm.envString("NOCK_SYMBOL");

        vm.startBroadcast();

        Nock nock = new Nock(nockName, nockSymbol, address(0));
        MessageInbox inboxImplementation = new MessageInbox();

        bytes memory initData = abi.encodeCall(
            MessageInbox.initialize,
            (bridgeNodes, address(nock))
        );

        ERC1967Proxy proxy = new ERC1967Proxy(address(inboxImplementation), initData);
        MessageInbox inbox = MessageInbox(address(proxy));

        nock.updateInbox(address(inbox));

        vm.stopBroadcast();

        _persistDeployment(
            address(inboxImplementation),
            address(inbox),
            address(nock)
        );
    }

    function _loadBridgeNodes() internal view returns (address[5] memory nodes) {
        nodes[0] = vm.envAddress("BRIDGE_NODE_0");
        nodes[1] = vm.envAddress("BRIDGE_NODE_1");
        nodes[2] = vm.envAddress("BRIDGE_NODE_2");
        nodes[3] = vm.envAddress("BRIDGE_NODE_3");
        nodes[4] = vm.envAddress("BRIDGE_NODE_4");
    }

    function _persistDeployment(
        address implementation,
        address proxy,
        address nock
    ) internal {
        string memory root = "deployment";
        string memory network = vm.envOr("DEPLOY_TARGET_NETWORK", string("tenderly-devnet"));
        address deployer = vm.envOr("DEPLOYER_ADDRESS", address(0));
        string memory path = vm.envOr("DEPLOYMENTS_PATH", string("deployments.json"));

        vm.serializeAddress(root, "messageInboxImplementation", implementation);
        vm.serializeAddress(root, "messageInboxProxy", proxy);
        vm.serializeAddress(root, "nock", nock);
        vm.serializeAddress(root, "deployer", deployer);
        vm.writeJson(vm.serializeString(root, "network", network), path);

        console2.log("MessageInbox implementation:", implementation);
        console2.log("MessageInbox proxy:", proxy);
        console2.log("Nock token:", nock);
        console2.log("Deployment metadata written to:", path);
    }
}

