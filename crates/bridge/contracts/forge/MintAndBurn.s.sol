// SPDX-License-Identifier: MIT
pragma solidity ^0.8.19;

import "forge-std/Script.sol";
import "forge-std/StdJson.sol";
import "forge-std/console2.sol";

import {Nock} from "../Nock.sol";
import {MessageInbox, Tip5Hash} from "../MessageInbox.sol";

contract MintAndBurn is Script {
    using stdJson for string;

    bytes32 private constant LOCK_ROOT =
        keccak256("wrapped-nock-test-lock-root");
    uint256 private constant AMOUNT = 1_000 * 1e16; // 1000 NOCK with 16 decimals
    uint256 private constant THRESHOLD = 3;

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

        address nockAddress = deploymentsFile.readAddress("$.nock");
        address inboxAddress = deploymentsFile.readAddress(
            "$.messageInboxProxy"
        );

        Nock nock = Nock(nockAddress);
        MessageInbox inbox = MessageInbox(inboxAddress);

        require(nock.inbox() == inboxAddress, "MintAndBurn: inbox mismatch");

        // Load bridge node private keys for signature generation
        uint256[THRESHOLD] memory bridgeKeys;
        for (uint256 i = 0; i < THRESHOLD; i++) {
            string memory keyName = string.concat(
                "BRIDGE_NODE_KEY_",
                vm.toString(i)
            );
            uint256 pk = vm.envOr(keyName, uint256(0));
            require(pk != 0, string.concat("MintAndBurn: set ", keyName));
            bridgeKeys[i] = pk;
            require(
                inbox.bridgeNodes(i) == vm.addr(pk),
                string.concat(
                    "MintAndBurn: bridge node mismatch at index ",
                    vm.toString(i)
                )
            );
        }

        uint256 holderPrivateKey = vm.envUint("NOCK_HOLDER_PRIVATE_KEY");
        address holder = vm.addr(holderPrivateKey);

        bytes32 txIdSeed = keccak256(abi.encodePacked(block.timestamp, holder, AMOUNT));
        Tip5Hash memory txId;
        txId.limbs[0] = uint64(uint256(txIdSeed) >> 192);
        txId.limbs[1] = uint64(uint256(txIdSeed) >> 128);
        txId.limbs[2] = uint64(uint256(txIdSeed) >> 64);
        txId.limbs[3] = uint64(uint256(txIdSeed));
        txId.limbs[4] = 0;

        uint256 blockHeight = block.number;

        bytes32 noteNameSeed = keccak256(abi.encodePacked("test-bundle", txIdSeed));
        Tip5Hash memory noteName;
        noteName.limbs[0] = uint64(uint256(noteNameSeed) >> 192);
        noteName.limbs[1] = uint64(uint256(noteNameSeed) >> 128);
        noteName.limbs[2] = uint64(uint256(noteNameSeed) >> 64);
        noteName.limbs[3] = uint64(uint256(noteNameSeed));
        noteName.limbs[4] = 0;

        bytes32 asOfSeed = keccak256(abi.encodePacked("test-as-of", blockHeight));
        Tip5Hash memory asOf;
        asOf.limbs[0] = uint64(uint256(asOfSeed) >> 192);
        asOf.limbs[1] = uint64(uint256(asOfSeed) >> 128);
        asOf.limbs[2] = uint64(uint256(asOfSeed) >> 64);
        asOf.limbs[3] = uint64(uint256(asOfSeed));
        asOf.limbs[4] = 0;

        bytes memory part1 = abi.encodePacked(txId.limbs[0], txId.limbs[1], txId.limbs[2], txId.limbs[3], txId.limbs[4]);
        bytes memory part2 = abi.encodePacked(noteName.limbs[0], noteName.limbs[1], noteName.limbs[2], noteName.limbs[3], noteName.limbs[4]);
        bytes memory part3 = abi.encodePacked(holder, AMOUNT, blockHeight);
        bytes memory part4 = abi.encodePacked(asOf.limbs[0], asOf.limbs[1], asOf.limbs[2], asOf.limbs[3], asOf.limbs[4]);
        bytes32 messageHash = keccak256(abi.encodePacked(part1, part2, part3, part4));
        bytes32 ethSignedMessageHash = keccak256(
            abi.encodePacked("\x19Ethereum Signed Message:\n32", messageHash)
        );

        bytes[] memory ethSigs = new bytes[](THRESHOLD);
        for (uint256 i = 0; i < THRESHOLD; i++) {
            (uint8 v, bytes32 r, bytes32 s) = vm.sign(
                bridgeKeys[i],
                ethSignedMessageHash
            );
            ethSigs[i] = _encodeSignature(v, r, s);
        }

        console2.log(
            "Submitting deposit for",
            AMOUNT / 1e16,
            "NOCK to",
            holder
        );
        vm.startBroadcast(bridgeKeys[0]);
        inbox.submitDeposit(
            txId,
            noteName,
            holder,
            AMOUNT,
            blockHeight,
            asOf,
            ethSigs
        );
        vm.stopBroadcast();

        console2.log("Holder balance after deposit", nock.balanceOf(holder));

        vm.startBroadcast(holderPrivateKey);
        nock.burn(AMOUNT, LOCK_ROOT);
        vm.stopBroadcast();

        console2.log("Holder balance after burn", nock.balanceOf(holder));
    }

    function _encodeSignature(
        uint8 v,
        bytes32 r,
        bytes32 s
    ) internal pure returns (bytes memory) {
        return abi.encodePacked(r, s, v);
    }
}
