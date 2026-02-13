// SPDX-License-Identifier: MIT
pragma solidity ^0.8.19;

import {Test} from "forge-std/Test.sol";

import {MessageInbox, Tip5Hash} from "../MessageInbox.sol";
import {Nock} from "../Nock.sol";

abstract contract BridgeTestBase is Test {
    MessageInbox internal inbox;
    Nock internal nock;

    uint256[5] internal bridgeNodePrivateKeys;
    address[5] internal bridgeNodeAddresses;

    uint256 internal constant ONE_NOCK = 1e16;
    uint64 internal constant PRIME = 0xffffffff00000001;

    uint256 internal nextDepositNonce = 1;

    function setUp() public virtual {
        _initBridgeNodes();
        _deployContracts();
    }

    function _initBridgeNodes() internal {
        for (uint256 i = 0; i < 5; i++) {
            uint256 pk = uint256(keccak256(abi.encodePacked("bridge-node", i)));
            bridgeNodePrivateKeys[i] = pk;
            bridgeNodeAddresses[i] = vm.addr(pk);
        }
    }

    function _deployContracts() internal {
        inbox = new MessageInbox();
        nock = new Nock("Nock", "NOCK", address(inbox));

        address[5] memory nodes;
        for (uint256 i = 0; i < 5; i++) {
            nodes[i] = bridgeNodeAddresses[i];
        }
        inbox.initialize(nodes, address(nock));
    }

    function bridgeNode(uint256 index) internal view returns (address) {
        require(index < 5, "bridge node index out of range");
        return bridgeNodeAddresses[index];
    }

    function _b32ToTip5(bytes32 value) internal pure returns (Tip5Hash memory h) {
        h.limbs[0] = uint64(uint256(value) >> 192) % PRIME;
        h.limbs[1] = uint64(uint256(value) >> 128) % PRIME;
        h.limbs[2] = uint64(uint256(value) >> 64) % PRIME;
        h.limbs[3] = uint64(uint256(value)) % PRIME;
        h.limbs[4] = 1; // Non-zero to avoid _isZeroTip5 rejection for asOf
    }

    function _depositMessageHash(
        Tip5Hash memory txId,
        Tip5Hash memory nameFirst,
        Tip5Hash memory nameLast,
        address recipient,
        uint256 amount,
        uint256 blockHeight,
        Tip5Hash memory asOf,
        uint256 depositNonce
    ) internal pure returns (bytes32) {
        bytes memory part1 = abi.encodePacked(txId.limbs[0], txId.limbs[1], txId.limbs[2], txId.limbs[3], txId.limbs[4]);
        bytes memory part2 = abi.encodePacked(nameFirst.limbs[0], nameFirst.limbs[1], nameFirst.limbs[2], nameFirst.limbs[3], nameFirst.limbs[4]);
        bytes memory part3 = abi.encodePacked(nameLast.limbs[0], nameLast.limbs[1], nameLast.limbs[2], nameLast.limbs[3], nameLast.limbs[4]);
        bytes memory part4 = abi.encodePacked(recipient, amount, blockHeight);
        bytes memory part5 = abi.encodePacked(asOf.limbs[0], asOf.limbs[1], asOf.limbs[2], asOf.limbs[3], asOf.limbs[4]);
        return keccak256(abi.encodePacked(part1, part2, part3, part4, part5, depositNonce));
    }

    function _ethSignedDepositHash(
        Tip5Hash memory txId,
        Tip5Hash memory nameFirst,
        Tip5Hash memory nameLast,
        address recipient,
        uint256 amount,
        uint256 blockHeight,
        Tip5Hash memory asOf,
        uint256 depositNonce
    ) internal pure returns (bytes32) {
        bytes32 messageHash = _depositMessageHash(txId, nameFirst, nameLast, recipient, amount, blockHeight, asOf, depositNonce);
        return keccak256(abi.encodePacked("\x19Ethereum Signed Message:\n32", messageHash));
    }

    function buildDepositSignature(
        uint256 nodeIndex,
        Tip5Hash memory txId,
        Tip5Hash memory nameFirst,
        Tip5Hash memory nameLast,
        address recipient,
        uint256 amount,
        uint256 blockHeight,
        Tip5Hash memory asOf,
        uint256 depositNonce
    ) internal view returns (bytes memory) {
        require(nodeIndex < 5, "bridge node index out of range");
        bytes32 digest = _ethSignedDepositHash(txId, nameFirst, nameLast, recipient, amount, blockHeight, asOf, depositNonce);
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(bridgeNodePrivateKeys[nodeIndex], digest);
        return abi.encodePacked(r, s, v);
    }

    function buildDepositSignatureSet(
        uint256[] memory nodeIndexes,
        Tip5Hash memory txId,
        Tip5Hash memory nameFirst,
        Tip5Hash memory nameLast,
        address recipient,
        uint256 amount,
        uint256 blockHeight,
        Tip5Hash memory asOf,
        uint256 depositNonce
    ) internal view returns (bytes[] memory sigs) {
        sigs = new bytes[](nodeIndexes.length);
        for (uint256 i = 0; i < nodeIndexes.length; i++) {
            sigs[i] = buildDepositSignature(nodeIndexes[i], txId, nameFirst, nameLast, recipient, amount, blockHeight, asOf, depositNonce);
        }
    }

    function _nextDepositNonce() internal returns (uint256) {
        return nextDepositNonce++;
    }

    function _defaultSignerIndexes() internal pure returns (uint256[] memory) {
        uint256[] memory signerIndexes = new uint256[](3);
        signerIndexes[0] = 0;
        signerIndexes[1] = 1;
        signerIndexes[2] = 2;
        return signerIndexes;
    }

    function mintFromInbox(address to, uint256 amount) internal {
        vm.prank(address(inbox));
        nock.mint(to, amount);
    }

    function bridgeNodesArray()
        internal
        view
        returns (address[5] memory nodes)
    {
        for (uint256 i = 0; i < 5; i++) {
            nodes[i] = bridgeNodeAddresses[i];
        }
    }

    function nockAmount(uint256 amount) internal pure returns (uint256) {
        return amount * ONE_NOCK;
    }
}
