// SPDX-License-Identifier: MIT
pragma solidity ^0.8.19;

import {OwnableUpgradeable} from "@openzeppelin/contracts-upgradeable/access/OwnableUpgradeable.sol";
import {MessageInbox, Tip5Hash} from "../MessageInbox.sol";
import {BridgeTestBase} from "./BridgeTestBase.t.sol";

contract MessageInboxDepositTest is BridgeTestBase {
    uint256 private constant SECP256K1_HALF_N =
        0x7FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF5D576E7357A4501DDFE92F46681B20A0;

    function setUp() public override {
        super.setUp();
    }

    function testSubmitDepositHappyPath() public {
        Tip5Hash memory txId = _b32ToTip5(keccak256("tx-id"));
        Tip5Hash memory nameFirst = _b32ToTip5(keccak256("name-first"));
        Tip5Hash memory nameLast = _b32ToTip5(keccak256("name-last"));
        address recipient = makeAddr("recipient");
        uint256 amount = nockAmount(1000);
        uint256 blockHeight = 42;
        Tip5Hash memory asOf = _b32ToTip5(keccak256("hashchain-tip"));
        uint256 depositNonce = _nextDepositNonce();

        uint256[] memory signerIndexes = new uint256[](3);
        signerIndexes[0] = 0;
        signerIndexes[1] = 1;
        signerIndexes[2] = 2;

        vm.pauseGasMetering();
        bytes[] memory sigs = buildDepositSignatureSet(
            signerIndexes,
            txId,
            nameFirst,
            nameLast,
            recipient,
            amount,
            blockHeight,
            asOf,
            depositNonce
        );
        vm.resumeGasMetering();

        bytes32 txIdHash = keccak256(abi.encodePacked(txId.limbs[0], txId.limbs[1], txId.limbs[2], txId.limbs[3], txId.limbs[4]));
        bytes32 nameFirstHash = keccak256(abi.encodePacked(nameFirst.limbs[0], nameFirst.limbs[1], nameFirst.limbs[2], nameFirst.limbs[3], nameFirst.limbs[4]));

        vm.expectEmit(true, true, true, true, address(inbox));
        emit MessageInbox.DepositProcessed(
            txIdHash,
            nameFirstHash,
            recipient,
            txId,
            nameFirst,
            nameLast,
            amount,
            blockHeight,
            asOf,
            depositNonce
        );

        inbox.submitDeposit(txId, nameFirst, nameLast, recipient, amount, blockHeight, asOf, depositNonce, sigs);

        assertTrue(inbox.processedDeposits(txIdHash));
        assertEq(nock.balanceOf(recipient), amount);
        assertEq(inbox.lastDepositNonce(), depositNonce);
    }

    function testSubmitDepositGasBelowTarget() public {
        Tip5Hash memory txId = _b32ToTip5(keccak256("gas-cap"));
        Tip5Hash memory nameFirst = _b32ToTip5(keccak256("name-first"));
        Tip5Hash memory nameLast = _b32ToTip5(keccak256("name-last"));
        address recipient = makeAddr("recipient");
        uint256 amount = nockAmount(500);
        uint256 blockHeight = 64;
        Tip5Hash memory asOf = _b32ToTip5(keccak256("gas-tip"));
        uint256 depositNonce = _nextDepositNonce();

        bytes[] memory sigs = buildDepositSignatureSet(
            _defaultSignerIndexes(),
            txId,
            nameFirst,
            nameLast,
            recipient,
            amount,
            blockHeight,
            asOf,
            depositNonce
        );

        uint256 gasBefore = gasleft();
        inbox.submitDeposit(txId, nameFirst, nameLast, recipient, amount, blockHeight, asOf, depositNonce, sigs);
        uint256 gasAfter = gasleft();

        uint256 gasUsed;
        unchecked {
            gasUsed = gasBefore - gasAfter;
        }

        assertLt(gasUsed, 200_000, "submitDeposit gas regression");
    }

    function testSubmitDepositRequiresThresholdSignatures() public {
        Tip5Hash memory txId = _b32ToTip5(keccak256("threshold"));
        Tip5Hash memory nameFirst = _b32ToTip5(keccak256("name-first"));
        Tip5Hash memory nameLast = _b32ToTip5(keccak256("name-last"));
        address recipient = makeAddr("recipient");
        uint256 amount = nockAmount(1);
        uint256 blockHeight = 1;
        Tip5Hash memory asOf = _b32ToTip5(keccak256("as-of"));
        uint256 depositNonce = _nextDepositNonce();

        bytes[] memory sigs = new bytes[](2);
        vm.expectRevert("Insufficient Ethereum signatures");
        inbox.submitDeposit(txId, nameFirst, nameLast, recipient, amount, blockHeight, asOf, depositNonce, sigs);
    }

    function testSubmitDepositRejectsDuplicateSigners() public {
        Tip5Hash memory txId = _b32ToTip5(keccak256("duplicate"));
        Tip5Hash memory nameFirst = _b32ToTip5(keccak256("name-first"));
        Tip5Hash memory nameLast = _b32ToTip5(keccak256("name-last"));
        address recipient = makeAddr("recipient");
        uint256 amount = nockAmount(2);
        uint256 blockHeight = 2;
        Tip5Hash memory asOf = _b32ToTip5(keccak256("as-of"));
        uint256 depositNonce = _nextDepositNonce();

        uint256[] memory signerIndexes = new uint256[](3);
        signerIndexes[0] = 0;
        signerIndexes[1] = 1;
        signerIndexes[2] = 1; // duplicate signer

        bytes[] memory sigs = buildDepositSignatureSet(
            signerIndexes,
            txId,
            nameFirst,
            nameLast,
            recipient,
            amount,
            blockHeight,
            asOf,
            depositNonce
        );

        vm.expectRevert("Invalid Ethereum signatures");
        inbox.submitDeposit(txId, nameFirst, nameLast, recipient, amount, blockHeight, asOf, depositNonce, sigs);
    }

    function testSubmitDepositRejectsMismatchedAsOf() public {
        Tip5Hash memory signedAsOf = _b32ToTip5(keccak256("signed"));
        Tip5Hash memory providedAsOf = _b32ToTip5(keccak256("provided"));
        Tip5Hash memory txId = _b32ToTip5(keccak256("asof-mismatch"));
        Tip5Hash memory nameFirst = _b32ToTip5(keccak256("name-first"));
        Tip5Hash memory nameLast = _b32ToTip5(keccak256("name-last"));
        address recipient = makeAddr("recipient");
        uint256 depositNonce = _nextDepositNonce();

        bytes[] memory sigs = buildDepositSignatureSet(
            _defaultSignerIndexes(),
            txId,
            nameFirst,
            nameLast,
            recipient,
            nockAmount(4),
            4,
            signedAsOf,
            depositNonce
        );

        vm.expectRevert("Invalid Ethereum signatures");
        inbox.submitDeposit(txId, nameFirst, nameLast, recipient, nockAmount(4), 4, providedAsOf, depositNonce, sigs);
    }

    function testSubmitDepositRejectsNonBridgeSigner() public {
        Tip5Hash memory txId = _b32ToTip5(keccak256("outsider"));
        Tip5Hash memory nameFirst = _b32ToTip5(keccak256("name-first"));
        Tip5Hash memory nameLast = _b32ToTip5(keccak256("name-last"));
        address recipient = makeAddr("recipient");
        uint256 amount = nockAmount(3);
        uint256 blockHeight = 3;
        Tip5Hash memory asOf = _b32ToTip5(keccak256("as-of"));
        uint256 depositNonce = _nextDepositNonce();

        bytes[] memory sigs = new bytes[](3);
        uint256[] memory signerIndexes = new uint256[](2);
        signerIndexes[0] = 0;
        signerIndexes[1] = 1;

        bytes[] memory legit = buildDepositSignatureSet(
            signerIndexes,
            txId,
            nameFirst,
            nameLast,
            recipient,
            amount,
            blockHeight,
            asOf,
            depositNonce
        );
        sigs[0] = legit[0];
        sigs[1] = legit[1];

        uint256 outsiderPk = uint256(keccak256("outsider-pk"));
        bytes32 digest = _ethSignedDepositHash(
            txId,
            nameFirst,
            nameLast,
            recipient,
            amount,
            blockHeight,
            asOf,
            depositNonce
        );
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(outsiderPk, digest);
        sigs[2] = abi.encodePacked(r, s, v);

        vm.expectRevert("Invalid Ethereum signatures");
        inbox.submitDeposit(txId, nameFirst, nameLast, recipient, amount, blockHeight, asOf, depositNonce, sigs);
    }

    function testSubmitDepositRejectsZeroRecipient() public {
        uint256 depositNonce = _nextDepositNonce();
        bytes[] memory sigs = new bytes[](3);
        vm.expectRevert("Invalid recipient");
        inbox.submitDeposit(
            _b32ToTip5(keccak256("tx")),
            _b32ToTip5(keccak256("name-first")),
            _b32ToTip5(keccak256("name-last")),
            address(0),
            nockAmount(1),
            1,
            _b32ToTip5(keccak256("asof")),
            depositNonce,
            sigs
        );
    }

    function testSubmitDepositRejectsZeroAmount() public {
        uint256 depositNonce = _nextDepositNonce();
        bytes[] memory sigs = new bytes[](3);
        vm.expectRevert("Amount must be positive");
        inbox.submitDeposit(
            _b32ToTip5(keccak256("tx")),
            _b32ToTip5(keccak256("name-first")),
            _b32ToTip5(keccak256("name-last")),
            makeAddr("recipient"),
            0,
            1,
            _b32ToTip5(keccak256("asof")),
            depositNonce,
            sigs
        );
    }

    function testSubmitDepositRejectsInvalidTip5() public {
        Tip5Hash memory invalid;
        invalid.limbs[0] = PRIME;
        uint256 depositNonce = _nextDepositNonce();

        bytes[] memory sigs = new bytes[](3);
        vm.expectRevert("Invalid txId");
        inbox.submitDeposit(
            invalid,
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

    function testSubmitDepositRejectsZeroAsOf() public {
        Tip5Hash memory zeroHash;
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
            zeroHash,
            depositNonce,
            sigs
        );
    }

    function testSubmitDepositRejectsDuplicateTxId() public {
        Tip5Hash memory txId = _b32ToTip5(keccak256("dupe"));
        Tip5Hash memory nameFirst = _b32ToTip5(keccak256("name-first"));
        Tip5Hash memory nameLast = _b32ToTip5(keccak256("name-last"));
        address recipient = makeAddr("recipient");
        uint256 amount = nockAmount(5);
        uint256 blockHeight = 5;
        Tip5Hash memory asOf = _b32ToTip5(keccak256("as-of"));
        uint256 depositNonce1 = _nextDepositNonce();

        uint256[] memory signerIndexes = new uint256[](3);
        signerIndexes[0] = 0;
        signerIndexes[1] = 1;
        signerIndexes[2] = 2;
        bytes[] memory sigs = buildDepositSignatureSet(
            signerIndexes,
            txId,
            nameFirst,
            nameLast,
            recipient,
            amount,
            blockHeight,
            asOf,
            depositNonce1
        );

        inbox.submitDeposit(txId, nameFirst, nameLast, recipient, amount, blockHeight, asOf, depositNonce1, sigs);

        uint256 depositNonce2 = _nextDepositNonce();
        bytes[] memory sigs2 = buildDepositSignatureSet(
            signerIndexes,
            txId,
            nameFirst,
            nameLast,
            recipient,
            amount,
            blockHeight,
            asOf,
            depositNonce2
        );

        vm.expectRevert("Deposit already processed");
        inbox.submitDeposit(txId, nameFirst, nameLast, recipient, amount, blockHeight, asOf, depositNonce2, sigs2);
    }

    function testSubmitDepositRejectsStaleNonce() public {
        Tip5Hash memory txId1 = _b32ToTip5(keccak256("tx1"));
        Tip5Hash memory nameFirst = _b32ToTip5(keccak256("name-first"));
        Tip5Hash memory nameLast = _b32ToTip5(keccak256("name-last"));
        address recipient = makeAddr("recipient");
        uint256 amount = nockAmount(1);
        uint256 blockHeight = 1;
        Tip5Hash memory asOf = _b32ToTip5(keccak256("as-of"));
        uint256 nonce10 = 10;

        bytes[] memory sigs1 = buildDepositSignatureSet(
            _defaultSignerIndexes(),
            txId1,
            nameFirst,
            nameLast,
            recipient,
            amount,
            blockHeight,
            asOf,
            nonce10
        );

        inbox.submitDeposit(txId1, nameFirst, nameLast, recipient, amount, blockHeight, asOf, nonce10, sigs1);
        assertEq(inbox.lastDepositNonce(), 10);

        Tip5Hash memory txId2 = _b32ToTip5(keccak256("tx2"));
        uint256 nonce5 = 5;

        bytes[] memory sigs2 = buildDepositSignatureSet(
            _defaultSignerIndexes(),
            txId2,
            nameFirst,
            nameLast,
            recipient,
            amount,
            blockHeight,
            asOf,
            nonce5
        );

        vm.expectRevert("Nonce must be strictly greater");
        inbox.submitDeposit(txId2, nameFirst, nameLast, recipient, amount, blockHeight, asOf, nonce5, sigs2);
    }

    function testSubmitDepositRejectsEqualNonce() public {
        Tip5Hash memory txId1 = _b32ToTip5(keccak256("tx-eq1"));
        Tip5Hash memory nameFirst = _b32ToTip5(keccak256("name-first"));
        Tip5Hash memory nameLast = _b32ToTip5(keccak256("name-last"));
        address recipient = makeAddr("recipient");
        uint256 amount = nockAmount(1);
        uint256 blockHeight = 1;
        Tip5Hash memory asOf = _b32ToTip5(keccak256("as-of"));
        uint256 nonce10 = 10;

        bytes[] memory sigs1 = buildDepositSignatureSet(
            _defaultSignerIndexes(),
            txId1,
            nameFirst,
            nameLast,
            recipient,
            amount,
            blockHeight,
            asOf,
            nonce10
        );

        inbox.submitDeposit(txId1, nameFirst, nameLast, recipient, amount, blockHeight, asOf, nonce10, sigs1);

        Tip5Hash memory txId2 = _b32ToTip5(keccak256("tx-eq2"));

        bytes[] memory sigs2 = buildDepositSignatureSet(
            _defaultSignerIndexes(),
            txId2,
            nameFirst,
            nameLast,
            recipient,
            amount,
            blockHeight,
            asOf,
            nonce10
        );

        vm.expectRevert("Nonce must be strictly greater");
        inbox.submitDeposit(txId2, nameFirst, nameLast, recipient, amount, blockHeight, asOf, nonce10, sigs2);
    }

    function testSubmitDepositAllowsNonContiguousNonces() public {
        Tip5Hash memory txId1 = _b32ToTip5(keccak256("tx-nc1"));
        Tip5Hash memory nameFirst = _b32ToTip5(keccak256("name-first"));
        Tip5Hash memory nameLast = _b32ToTip5(keccak256("name-last"));
        address recipient = makeAddr("recipient");
        uint256 amount = nockAmount(1);
        uint256 blockHeight = 1;
        Tip5Hash memory asOf = _b32ToTip5(keccak256("as-of"));

        bytes[] memory sigs1 = buildDepositSignatureSet(
            _defaultSignerIndexes(),
            txId1,
            nameFirst,
            nameLast,
            recipient,
            amount,
            blockHeight,
            asOf,
            5
        );

        inbox.submitDeposit(txId1, nameFirst, nameLast, recipient, amount, blockHeight, asOf, 5, sigs1);
        assertEq(inbox.lastDepositNonce(), 5);

        Tip5Hash memory txId2 = _b32ToTip5(keccak256("tx-nc2"));

        bytes[] memory sigs2 = buildDepositSignatureSet(
            _defaultSignerIndexes(),
            txId2,
            nameFirst,
            nameLast,
            recipient,
            amount,
            blockHeight,
            asOf,
            100
        );

        inbox.submitDeposit(txId2, nameFirst, nameLast, recipient, amount, blockHeight, asOf, 100, sigs2);
        assertEq(inbox.lastDepositNonce(), 100);
    }

    function testSubmitDepositRejectsMalleatedSignature() public {
        Tip5Hash memory txId = _b32ToTip5(keccak256("malleated"));
        Tip5Hash memory nameFirst = _b32ToTip5(keccak256("name-first"));
        Tip5Hash memory nameLast = _b32ToTip5(keccak256("name-last"));
        address recipient = makeAddr("recipient");
        uint256 amount = nockAmount(6);
        uint256 blockHeight = 6;
        Tip5Hash memory asOf = _b32ToTip5(keccak256("tip"));
        uint256 depositNonce = _nextDepositNonce();

        bytes[] memory sigs = buildDepositSignatureSet(
            _defaultSignerIndexes(),
            txId,
            nameFirst,
            nameLast,
            recipient,
            amount,
            blockHeight,
            asOf,
            depositNonce
        );

        (bytes32 r, , uint8 v) = _signatureComponents(sigs[0]);
        bytes32 highS = bytes32(SECP256K1_HALF_N + 1);
        sigs[0] = _repackSignature(r, highS, v);

        vm.expectRevert("Invalid signature s value");
        inbox.submitDeposit(txId, nameFirst, nameLast, recipient, amount, blockHeight, asOf, depositNonce, sigs);
    }

    function testSubmitDepositRejectsInvalidSignatureV() public {
        Tip5Hash memory txId = _b32ToTip5(keccak256("invalid-v"));
        Tip5Hash memory nameFirst = _b32ToTip5(keccak256("name-first"));
        Tip5Hash memory nameLast = _b32ToTip5(keccak256("name-last"));
        address recipient = makeAddr("recipient");
        uint256 amount = nockAmount(7);
        uint256 blockHeight = 7;
        Tip5Hash memory asOf = _b32ToTip5(keccak256("tip"));
        uint256 depositNonce = _nextDepositNonce();

        bytes[] memory sigs = buildDepositSignatureSet(
            _defaultSignerIndexes(),
            txId,
            nameFirst,
            nameLast,
            recipient,
            amount,
            blockHeight,
            asOf,
            depositNonce
        );

        (bytes32 r, bytes32 s, ) = _signatureComponents(sigs[0]);
        sigs[0] = _repackSignature(r, s, 29);

        vm.expectRevert("Invalid signature v value");
        inbox.submitDeposit(txId, nameFirst, nameLast, recipient, amount, blockHeight, asOf, depositNonce, sigs);
    }

    function testSubmitDepositRejectsInvalidSignatureLength() public {
        Tip5Hash memory txId = _b32ToTip5(keccak256("sig-length"));
        Tip5Hash memory nameFirst = _b32ToTip5(keccak256("name-first"));
        Tip5Hash memory nameLast = _b32ToTip5(keccak256("name-last"));
        address recipient = makeAddr("recipient");
        uint256 amount = nockAmount(8);
        uint256 blockHeight = 8;
        Tip5Hash memory asOf = _b32ToTip5(keccak256("tip"));
        uint256 depositNonce = _nextDepositNonce();

        uint256[] memory signerIndexes = _defaultSignerIndexes();
        bytes[] memory sigs = buildDepositSignatureSet(
            signerIndexes,
            txId,
            nameFirst,
            nameLast,
            recipient,
            amount,
            blockHeight,
            asOf,
            depositNonce
        );

        sigs[0] = new bytes(64);

        vm.expectRevert("Invalid signature length");
        inbox.submitDeposit(txId, nameFirst, nameLast, recipient, amount, blockHeight, asOf, depositNonce, sigs);
    }

    function testUpdateBridgeNodeOnlyOwner() public {
        address newNode = makeAddr("new-node");

        vm.expectEmit(true, true, true, true, address(inbox));
        emit MessageInbox.BridgeNodeUpdated(0, bridgeNode(0), newNode);

        inbox.updateBridgeNode(0, newNode);
        assertEq(inbox.bridgeNodes(0), newNode);
    }

    function testUpdateBridgeNodeRejectsInvalidIndex() public {
        vm.expectRevert("Invalid bridge node index");
        inbox.updateBridgeNode(5, makeAddr("node"));
    }

    function testUpdateBridgeNodeRejectsZeroAddress() public {
        vm.expectRevert("Invalid bridge node address");
        inbox.updateBridgeNode(0, address(0));
    }

    function testUpdateBridgeNodeRequiresOwner() public {
        address attacker = makeAddr("attacker");
        vm.prank(attacker);
        vm.expectRevert(abi.encodeWithSelector(OwnableUpgradeable.OwnableUnauthorizedAccount.selector, attacker));
        inbox.updateBridgeNode(0, makeAddr("node"));
    }

    function testNotifyBurnOnlyNockCanCall() public {
        vm.expectRevert("Only Nock contract can notify burns");
        inbox.notifyBurn();
    }

    function _signatureComponents(
        bytes memory sig
    ) internal pure returns (bytes32 r, bytes32 s, uint8 v) {
        assembly {
            r := mload(add(sig, 32))
            s := mload(add(sig, 64))
            v := byte(0, mload(add(sig, 96)))
        }
    }

    function _repackSignature(
        bytes32 r,
        bytes32 s,
        uint8 v
    ) internal pure returns (bytes memory) {
        return abi.encodePacked(r, s, v);
    }
}
