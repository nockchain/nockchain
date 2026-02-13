// SPDX-License-Identifier: MIT
pragma solidity ^0.8.19;

import {Tip5Hash} from "../MessageInbox.sol";
import {BridgeTestBase} from "./BridgeTestBase.t.sol";

/// @notice Fuzz tests for MessageInbox signature validation and Tip5Hash handling
contract MessageInboxFuzzTest is BridgeTestBase {
    uint256 private constant SECP256K1_N =
        0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEBAAEDCE6AF48A03BBFD25E8CD0364141;
    uint256 private constant SECP256K1_HALF_N = SECP256K1_N / 2;
    uint64 private constant TIP5_PRIME = 0xffffffff00000001;

    function setUp() public override {
        super.setUp();
    }

    /// @notice Fuzz test: any signature with s > SECP256K1_N/2 should be rejected
    function testFuzz_rejectMalleatedSignatureS(uint256 sValue) public {
        // Bound s to be > HALF_N (malleated range)
        sValue = bound(sValue, SECP256K1_HALF_N + 1, SECP256K1_N - 1);

        Tip5Hash memory txId = _b32ToTip5(keccak256("fuzz-malleated"));
        Tip5Hash memory nameFirst = _b32ToTip5(keccak256("name-first"));
        Tip5Hash memory nameLast = _b32ToTip5(keccak256("name-last"));
        address recipient = makeAddr("recipient");
        uint256 amount = nockAmount(1);
        uint256 blockHeight = 1;
        Tip5Hash memory asOf = _b32ToTip5(keccak256("asof"));
        uint256 depositNonce = _nextDepositNonce();

        // Build valid signatures first
        uint256[] memory signerIndexes = new uint256[](3);
        signerIndexes[0] = 0;
        signerIndexes[1] = 1;
        signerIndexes[2] = 2;

        bytes[] memory sigs = buildDepositSignatureSet(
            signerIndexes, txId, nameFirst, nameLast, recipient, amount, blockHeight, asOf, depositNonce
        );

        // Extract r and v from first signature, replace s with malleated value
        bytes32 r;
        uint8 v;
        assembly {
            let sigPtr := mload(add(sigs, 32))
            r := mload(add(sigPtr, 32))
            v := byte(0, mload(add(sigPtr, 96)))
        }

        // Repack with malleated s
        sigs[0] = abi.encodePacked(r, bytes32(sValue), v);

        vm.expectRevert("Invalid signature s value");
        inbox.submitDeposit(txId, nameFirst, nameLast, recipient, amount, blockHeight, asOf, depositNonce, sigs);
    }

    /// @notice Fuzz test: any signature with invalid v (not 27 or 28) should be rejected
    function testFuzz_rejectInvalidSignatureV(uint8 vValue) public {
        vm.assume(vValue != 27 && vValue != 28);

        Tip5Hash memory txId = _b32ToTip5(keccak256("fuzz-invalid-v"));
        Tip5Hash memory nameFirst = _b32ToTip5(keccak256("name-first"));
        Tip5Hash memory nameLast = _b32ToTip5(keccak256("name-last"));
        address recipient = makeAddr("recipient");
        uint256 amount = nockAmount(1);
        uint256 blockHeight = 1;
        Tip5Hash memory asOf = _b32ToTip5(keccak256("asof"));
        uint256 depositNonce = _nextDepositNonce();

        uint256[] memory signerIndexes = new uint256[](3);
        signerIndexes[0] = 0;
        signerIndexes[1] = 1;
        signerIndexes[2] = 2;

        bytes[] memory sigs = buildDepositSignatureSet(
            signerIndexes, txId, nameFirst, nameLast, recipient, amount, blockHeight, asOf, depositNonce
        );

        // Extract r and s from first signature
        bytes32 r;
        bytes32 s;
        assembly {
            let sigPtr := mload(add(sigs, 32))
            r := mload(add(sigPtr, 32))
            s := mload(add(sigPtr, 64))
        }

        // Repack with invalid v
        sigs[0] = abi.encodePacked(r, s, vValue);

        vm.expectRevert("Invalid signature v value");
        inbox.submitDeposit(txId, nameFirst, nameLast, recipient, amount, blockHeight, asOf, depositNonce, sigs);
    }

    /// @notice Fuzz test: signatures with wrong length should be rejected
    function testFuzz_rejectInvalidSignatureLength(uint8 length) public {
        vm.assume(length != 65 && length < 200); // Avoid huge allocations

        Tip5Hash memory txId = _b32ToTip5(keccak256("fuzz-length"));
        Tip5Hash memory nameFirst = _b32ToTip5(keccak256("name-first"));
        Tip5Hash memory nameLast = _b32ToTip5(keccak256("name-last"));
        address recipient = makeAddr("recipient");
        uint256 amount = nockAmount(1);
        uint256 blockHeight = 1;
        Tip5Hash memory asOf = _b32ToTip5(keccak256("asof"));
        uint256 depositNonce = _nextDepositNonce();

        uint256[] memory signerIndexes = new uint256[](3);
        signerIndexes[0] = 0;
        signerIndexes[1] = 1;
        signerIndexes[2] = 2;

        bytes[] memory sigs = buildDepositSignatureSet(
            signerIndexes, txId, nameFirst, nameLast, recipient, amount, blockHeight, asOf, depositNonce
        );

        // Replace first signature with wrong length
        sigs[0] = new bytes(length);

        vm.expectRevert("Invalid signature length");
        inbox.submitDeposit(txId, nameFirst, nameLast, recipient, amount, blockHeight, asOf, depositNonce, sigs);
    }

    /// @notice Fuzz test: Tip5Hash with any limb >= PRIME should be rejected as txId
    function testFuzz_rejectInvalidTip5TxId(uint64 invalidLimb, uint8 limbIndex) public {
        vm.assume(invalidLimb >= TIP5_PRIME);
        limbIndex = limbIndex % 5; // Ensure valid index

        Tip5Hash memory txId;
        // Set valid values for all limbs first
        txId.limbs[0] = 1;
        txId.limbs[1] = 2;
        txId.limbs[2] = 3;
        txId.limbs[3] = 4;
        txId.limbs[4] = 5;
        // Then set the invalid limb
        txId.limbs[limbIndex] = invalidLimb;

        uint256 depositNonce = _nextDepositNonce();
        bytes[] memory sigs = new bytes[](3);
        vm.expectRevert("Invalid txId");
        inbox.submitDeposit(
            txId,
            _b32ToTip5(keccak256("name-first")),
            _b32ToTip5(keccak256("name-last")),
            makeAddr("recipient"),
            nockAmount(1),
            1,
            _b32ToTip5(keccak256("asof")),
            depositNonce,
            sigs
        );
    }

    /// @notice Fuzz test: Tip5Hash with any limb >= PRIME should be rejected as nameFirst
    function testFuzz_rejectInvalidTip5NameFirst(uint64 invalidLimb, uint8 limbIndex) public {
        vm.assume(invalidLimb >= TIP5_PRIME);
        limbIndex = limbIndex % 5;

        Tip5Hash memory nameFirst;
        nameFirst.limbs[0] = 1;
        nameFirst.limbs[1] = 2;
        nameFirst.limbs[2] = 3;
        nameFirst.limbs[3] = 4;
        nameFirst.limbs[4] = 5;
        nameFirst.limbs[limbIndex] = invalidLimb;

        uint256 depositNonce = _nextDepositNonce();
        bytes[] memory sigs = new bytes[](3);
        vm.expectRevert("Invalid nameFirst");
        inbox.submitDeposit(
            _b32ToTip5(keccak256("tx")),
            nameFirst,
            _b32ToTip5(keccak256("name-last")),
            makeAddr("recipient"),
            nockAmount(1),
            1,
            _b32ToTip5(keccak256("asof")),
            depositNonce,
            sigs
        );
    }

    /// @notice Fuzz test: Tip5Hash with any limb >= PRIME should be rejected as nameLast
    function testFuzz_rejectInvalidTip5NameLast(uint64 invalidLimb, uint8 limbIndex) public {
        vm.assume(invalidLimb >= TIP5_PRIME);
        limbIndex = limbIndex % 5;

        Tip5Hash memory nameLast;
        nameLast.limbs[0] = 1;
        nameLast.limbs[1] = 2;
        nameLast.limbs[2] = 3;
        nameLast.limbs[3] = 4;
        nameLast.limbs[4] = 5;
        nameLast.limbs[limbIndex] = invalidLimb;

        uint256 depositNonce = _nextDepositNonce();
        bytes[] memory sigs = new bytes[](3);
        vm.expectRevert("Invalid nameLast");
        inbox.submitDeposit(
            _b32ToTip5(keccak256("tx")),
            _b32ToTip5(keccak256("name-first")),
            nameLast,
            makeAddr("recipient"),
            nockAmount(1),
            1,
            _b32ToTip5(keccak256("asof")),
            depositNonce,
            sigs
        );
    }

    /// @notice Fuzz test: Tip5Hash with any limb >= PRIME should be rejected as asOf
    function testFuzz_rejectInvalidTip5AsOf(uint64 invalidLimb, uint8 limbIndex) public {
        vm.assume(invalidLimb >= TIP5_PRIME);
        limbIndex = limbIndex % 5;

        Tip5Hash memory asOf;
        asOf.limbs[0] = 1;
        asOf.limbs[1] = 2;
        asOf.limbs[2] = 3;
        asOf.limbs[3] = 4;
        asOf.limbs[4] = 5;
        asOf.limbs[limbIndex] = invalidLimb;

        uint256 depositNonce = _nextDepositNonce();
        bytes[] memory sigs = new bytes[](3);
        vm.expectRevert("Invalid asOf");
        inbox.submitDeposit(
            _b32ToTip5(keccak256("tx")),
            _b32ToTip5(keccak256("name-first")),
            _b32ToTip5(keccak256("name-last")),
            makeAddr("recipient"),
            nockAmount(1),
            1,
            asOf,
            depositNonce,
            sigs
        );
    }

    /// @notice Test: zero Tip5Hash (all limbs = 0) should be rejected as asOf
    /// Note: This is not a fuzz test since we need all limbs to be exactly zero
    function test_rejectZeroAsOf() public {
        Tip5Hash memory asOf;
        // All limbs default to 0

        uint256 depositNonce = _nextDepositNonce();
        bytes[] memory sigs = new bytes[](3);
        vm.expectRevert("Invalid as-of hash");
        inbox.submitDeposit(
            _b32ToTip5(keccak256("tx")),
            _b32ToTip5(keccak256("name-first")),
            _b32ToTip5(keccak256("name-last")),
            makeAddr("recipient"),
            nockAmount(1),
            1,
            asOf,
            depositNonce,
            sigs
        );
    }

    /// @notice Fuzz test: valid Tip5Hash values should be accepted (sanity check)
    function testFuzz_acceptValidTip5(
        uint64 limb0, uint64 limb1, uint64 limb2, uint64 limb3, uint64 limb4
    ) public {
        // Bound all limbs to valid range
        limb0 = uint64(bound(limb0, 1, TIP5_PRIME - 1));
        limb1 = uint64(bound(limb1, 0, TIP5_PRIME - 1));
        limb2 = uint64(bound(limb2, 0, TIP5_PRIME - 1));
        limb3 = uint64(bound(limb3, 0, TIP5_PRIME - 1));
        limb4 = uint64(bound(limb4, 0, TIP5_PRIME - 1));

        Tip5Hash memory txId;
        txId.limbs[0] = limb0;
        txId.limbs[1] = limb1;
        txId.limbs[2] = limb2;
        txId.limbs[3] = limb3;
        txId.limbs[4] = limb4;

        Tip5Hash memory nameFirst = _b32ToTip5(keccak256("name-first"));
        Tip5Hash memory nameLast = _b32ToTip5(keccak256("name-last"));
        Tip5Hash memory asOf = _b32ToTip5(keccak256("asof"));
        address recipient = makeAddr("recipient");
        uint256 amount = nockAmount(1);
        uint256 blockHeight = 1;
        uint256 depositNonce = _nextDepositNonce();

        uint256[] memory signerIndexes = new uint256[](3);
        signerIndexes[0] = 0;
        signerIndexes[1] = 1;
        signerIndexes[2] = 2;

        bytes[] memory sigs = buildDepositSignatureSet(
            signerIndexes, txId, nameFirst, nameLast, recipient, amount, blockHeight, asOf, depositNonce
        );

        // Should succeed without reverting
        inbox.submitDeposit(txId, nameFirst, nameLast, recipient, amount, blockHeight, asOf, depositNonce, sigs);

        // Verify it was processed
        bytes32 txIdHash = keccak256(abi.encodePacked(
            txId.limbs[0], txId.limbs[1], txId.limbs[2], txId.limbs[3], txId.limbs[4]
        ));
        assertTrue(inbox.processedDeposits(txIdHash));
    }

    /// @notice Fuzz test: random bytes as signature should fail gracefully
    function testFuzz_rejectRandomSignatureBytes(bytes32 rand1, bytes32 rand2, uint8 rand3) public {
        Tip5Hash memory txId = _b32ToTip5(keccak256(abi.encodePacked("fuzz-random", rand1)));
        Tip5Hash memory nameFirst = _b32ToTip5(keccak256("name-first"));
        Tip5Hash memory nameLast = _b32ToTip5(keccak256("name-last"));
        address recipient = makeAddr("recipient");
        uint256 amount = nockAmount(1);
        uint256 blockHeight = 1;
        Tip5Hash memory asOf = _b32ToTip5(keccak256("asof"));
        uint256 depositNonce = _nextDepositNonce();

        bytes[] memory sigs = new bytes[](3);
        // First two are valid
        uint256[] memory signerIndexes = new uint256[](2);
        signerIndexes[0] = 0;
        signerIndexes[1] = 1;
        bytes[] memory validSigs = buildDepositSignatureSet(
            signerIndexes, txId, nameFirst, nameLast, recipient, amount, blockHeight, asOf, depositNonce
        );
        sigs[0] = validSigs[0];
        sigs[1] = validSigs[1];

        // Third is random garbage (but correct length)
        sigs[2] = abi.encodePacked(rand1, rand2, rand3);

        // Should revert with one of several possible errors depending on the random values
        // Either invalid s, invalid v, or invalid signatures (wrong signer)
        vm.expectRevert();
        inbox.submitDeposit(txId, nameFirst, nameLast, recipient, amount, blockHeight, asOf, depositNonce, sigs);
    }

    /// @notice Fuzz test: signature with s = 0 should be rejected
    function testFuzz_rejectZeroS() public {
        Tip5Hash memory txId = _b32ToTip5(keccak256("fuzz-zero-s"));
        Tip5Hash memory nameFirst = _b32ToTip5(keccak256("name-first"));
        Tip5Hash memory nameLast = _b32ToTip5(keccak256("name-last"));
        address recipient = makeAddr("recipient");
        uint256 amount = nockAmount(1);
        uint256 blockHeight = 1;
        Tip5Hash memory asOf = _b32ToTip5(keccak256("asof"));
        uint256 depositNonce = _nextDepositNonce();

        uint256[] memory signerIndexes = new uint256[](3);
        signerIndexes[0] = 0;
        signerIndexes[1] = 1;
        signerIndexes[2] = 2;

        bytes[] memory sigs = buildDepositSignatureSet(
            signerIndexes, txId, nameFirst, nameLast, recipient, amount, blockHeight, asOf, depositNonce
        );

        // Extract r and v, set s to 0
        bytes32 r;
        uint8 v;
        assembly {
            let sigPtr := mload(add(sigs, 32))
            r := mload(add(sigPtr, 32))
            v := byte(0, mload(add(sigPtr, 96)))
        }

        sigs[0] = abi.encodePacked(r, bytes32(0), v);

        vm.expectRevert("Invalid signature s value");
        inbox.submitDeposit(txId, nameFirst, nameLast, recipient, amount, blockHeight, asOf, depositNonce, sigs);
    }

    /// @notice Fuzz test: valid s values in low range should be accepted
    function testFuzz_acceptValidLowS(uint256 sValue) public {
        // Bound s to valid range: 0 < s <= HALF_N
        sValue = bound(sValue, 1, SECP256K1_HALF_N);

        // This test just verifies our bounds - actual signature verification
        // requires the s to match the actual signed message, so we can't
        // easily fuzz valid signatures. Instead, we verify the boundary logic
        // by checking that our test helper generates valid low-s signatures.

        Tip5Hash memory txId = _b32ToTip5(keccak256(abi.encodePacked("fuzz-valid-s", sValue)));
        Tip5Hash memory nameFirst = _b32ToTip5(keccak256("name-first"));
        Tip5Hash memory nameLast = _b32ToTip5(keccak256("name-last"));
        address recipient = makeAddr("recipient");
        uint256 amount = nockAmount(1);
        uint256 blockHeight = 1;
        Tip5Hash memory asOf = _b32ToTip5(keccak256("asof"));
        uint256 depositNonce = _nextDepositNonce();

        uint256[] memory signerIndexes = new uint256[](3);
        signerIndexes[0] = 0;
        signerIndexes[1] = 1;
        signerIndexes[2] = 2;

        bytes[] memory sigs = buildDepositSignatureSet(
            signerIndexes, txId, nameFirst, nameLast, recipient, amount, blockHeight, asOf, depositNonce
        );

        // Verify that our test helper generates low-s signatures
        bytes32 s;
        assembly {
            let sigPtr := mload(add(sigs, 32))
            s := mload(add(sigPtr, 64))
        }
        assertTrue(uint256(s) <= SECP256K1_HALF_N, "Test helper should generate low-s signatures");

        // Should succeed
        inbox.submitDeposit(txId, nameFirst, nameLast, recipient, amount, blockHeight, asOf, depositNonce, sigs);
    }
}
