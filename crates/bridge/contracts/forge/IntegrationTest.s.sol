// SPDX-License-Identifier: MIT
pragma solidity ^0.8.19;

import "forge-std/Script.sol";
import "forge-std/StdJson.sol";
import "forge-std/console2.sol";

import {MessageInbox, Tip5Hash} from "../MessageInbox.sol";
import {Nock} from "../Nock.sol";

/// @notice Integration test against deployed contracts
///         Tests full deposit + burn cycle
///         Validates gas usage stays under 200k for deposits
contract IntegrationTest is Script {
    function _b32ToTip5(bytes32 value) internal pure returns (Tip5Hash memory h) {
        h.limbs[0] = uint64(uint256(value) >> 192);
        h.limbs[1] = uint64(uint256(value) >> 128);
        h.limbs[2] = uint64(uint256(value) >> 64);
        h.limbs[3] = uint64(uint256(value));
        h.limbs[4] = 0;
    }
    using stdJson for string;

    uint256 private constant TEST_AMOUNT = 1_000 * 1e16;
    bytes32 private constant LOCK_ROOT =
        keccak256("integration-test-lock-root");
    uint256 private constant THRESHOLD = 3;
    uint256 private constant MAX_DEPOSIT_GAS = 200_000;

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
        address nock = deploymentsFile.readAddress("$.nock");

        MessageInbox inbox = MessageInbox(proxy);
        Nock nockToken = Nock(nock);

        uint256 testKey = vm.envUint("TEST_ACCOUNT_PRIVATE_KEY");
        address testAccount = vm.addr(testKey);

        uint256[THRESHOLD] memory bridgeKeys;
        for (uint256 i = 0; i < THRESHOLD; i++) {
            string memory keyName = string.concat(
                "BRIDGE_NODE_KEY_",
                vm.toString(i)
            );
            bridgeKeys[i] = vm.envUint(keyName);
            require(
                inbox.bridgeNodes(i) == vm.addr(bridgeKeys[i]),
                "Bridge node key mismatch"
            );
        }

        bytes32 txIdSeed = keccak256(abi.encodePacked(block.timestamp, testAccount, TEST_AMOUNT));
        Tip5Hash memory txId = _b32ToTip5(txIdSeed);
        uint256 blockHeight = block.number;
        Tip5Hash memory noteName = _b32ToTip5(keccak256(abi.encodePacked("integration-test", txIdSeed)));
        Tip5Hash memory asOf = _b32ToTip5(keccak256(abi.encodePacked("integration-as-of", blockHeight)));

        uint256 depositNonce = inbox.lastDepositNonce() + 1;

        bytes memory part1 = abi.encodePacked(txId.limbs[0], txId.limbs[1], txId.limbs[2], txId.limbs[3], txId.limbs[4]);
        bytes memory part2 = abi.encodePacked(noteName.limbs[0], noteName.limbs[1], noteName.limbs[2], noteName.limbs[3], noteName.limbs[4]);
        bytes memory part3 = abi.encodePacked(testAccount, TEST_AMOUNT, blockHeight);
        bytes memory part4 = abi.encodePacked(asOf.limbs[0], asOf.limbs[1], asOf.limbs[2], asOf.limbs[3], asOf.limbs[4]);
        bytes32 messageHash = keccak256(abi.encodePacked(part1, part2, part3, part4, depositNonce));
        bytes32 ethSignedMessageHash = keccak256(
            abi.encodePacked("\x19Ethereum Signed Message:\n32", messageHash)
        );

        bytes[] memory ethSigs = new bytes[](THRESHOLD);
        for (uint256 i = 0; i < THRESHOLD; i++) {
            (uint8 v, bytes32 r, bytes32 s) = vm.sign(
                bridgeKeys[i],
                ethSignedMessageHash
            );
            ethSigs[i] = abi.encodePacked(r, s, v);
        }

        uint256 gasBefore = gasleft();
        vm.startBroadcast(bridgeKeys[0]);
        inbox.submitDeposit(
            txId,
            noteName,
            testAccount,
            TEST_AMOUNT,
            blockHeight,
            asOf,
            depositNonce,
            ethSigs
        );
        vm.stopBroadcast();
        uint256 gasUsed = gasBefore - gasleft();

        require(
            gasUsed <= MAX_DEPOSIT_GAS,
            "Deposit gas exceeds 200k limit"
        );

        require(nockToken.balanceOf(testAccount) == TEST_AMOUNT, "Balance mismatch");

        vm.startBroadcast(testKey);
        nockToken.burn(TEST_AMOUNT, LOCK_ROOT);
        vm.stopBroadcast();

        require(nockToken.balanceOf(testAccount) == 0, "Balance not zero after burn");
    }
}

