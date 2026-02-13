// SPDX-License-Identifier: MIT
pragma solidity ^0.8.19;

import "@openzeppelin/contracts-upgradeable/proxy/utils/Initializable.sol";
import "@openzeppelin/contracts-upgradeable/proxy/utils/UUPSUpgradeable.sol";
import "@openzeppelin/contracts-upgradeable/access/OwnableUpgradeable.sol";
import "./Nock.sol";

struct Tip5Hash {
    uint64[5] limbs;
}

/**
 * @title MessageInbox
 * @dev Upgradeable contract for bridge message processing
 *
 * This contract receives deposits from bridge nodes and mints wrapped nock.
 * The contract is upgradeable but only the bridge multisig can upgrade it.
 */
contract MessageInbox is Initializable, UUPSUpgradeable, OwnableUpgradeable {
    string public constant VERSION = "1.0.0";

    // Enforce canonical ECDSA signatures: secp256k1 group order (N) defines the
    // valid range for `s`, and values above N/2 represent the malleated form of
    // the same signature. Rejecting s > N/2 and non-standard v (anything other
    // than 27/28) prevents attackers from replaying a mutated signature that
    // still passes ecrecover but points to the same signer.
    uint256 private constant SECP256K1_N =
        0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEBAAEDCE6AF48A03BBFD25E8CD0364141;
    uint256 private constant SECP256K1_HALF_N = SECP256K1_N / 2;

    // Bridge node addresses (5 multisig addresses)
    address[5] public bridgeNodes;

    // Signature threshold (3-of-5)
    uint256 public constant THRESHOLD = 3;

    /**
     * @dev Replay protection for deposit bundles.
     *
     * SECURITY CRITICAL: This mapping is the ONLY on-chain mechanism preventing
     * double-mint attacks. Bridge node signatures do not expire, so without this
     * check, any valid signed bundle could be resubmitted to mint tokens again.
     *
     * The off-chain Hoon state machine tracks deposits but cannot prevent replay
     * because it is not the final arbiter of token minting - this contract is.
     *
     * Key: keccak256 of Nockchain txId (Tip5Hash encoded as 5 uint64 limbs)
     * Value: true if this deposit has been processed
     */
    mapping(bytes32 => bool) public processedDeposits;
    uint256 public lastDepositNonce;

    // Withdrawals toggle: when false, burns on Nock.sol revert atomically
    // since notifyBurn is called during the burn transaction
    bool public withdrawalsEnabled;

    // Nock token contract
    Nock public nock;

    // Events
    event DepositProcessed(
        bytes32 indexed txId,
        bytes32 indexed nameFirstHash,
        address indexed recipient,
        Tip5Hash txIdFull,
        Tip5Hash nameFirst,
        Tip5Hash nameLast,
        uint256 amount,
        uint256 blockHeight,
        Tip5Hash asOf,
        uint256 nonce
    );

    event BridgeNodeUpdated(
        uint256 indexed index,
        address indexed oldNode,
        address indexed newNode
    );

    event WithdrawalsToggled(bool enabled);

    /**
     * @dev Initialize the contract
     * @param _bridgeNodes Array of 5 bridge node addresses
     * @param _nock Address of the Nock token contract
     */
    function initialize(
        address[5] memory _bridgeNodes,
        address _nock
    ) public initializer {
        __Ownable_init(msg.sender);

        // Validate all bridge nodes are non-zero and unique
        for (uint256 i = 0; i < 5; i++) {
            require(_bridgeNodes[i] != address(0), "Bridge node cannot be zero address");
            for (uint256 j = i + 1; j < 5; j++) {
                require(_bridgeNodes[i] != _bridgeNodes[j], "Duplicate bridge node address");
            }
        }

        bridgeNodes = _bridgeNodes;
        nock = Nock(_nock);
        withdrawalsEnabled = true;
    }

    /**
     * @dev Submit a deposit bundle from bridge nodes
     * @param txId Nockchain transaction ID (5 field elements)
     * @param nameFirst First component of Nockchain note name (5 field elements)
     * @param nameLast Last component of Nockchain note name (5 field elements)
     * @param recipient Ethereum recipient address
     * @param amount Amount of nock to mint (in nicks)
     * @param blockHeight Nockchain block height
     * @param asOf Hash establishing causality (5 field elements)
     * @param depositNonce Strictly monotonic nonce for event ordering
     * @param ethSigs Array of Ethereum signatures from bridge nodes
     */
    function submitDeposit(
        Tip5Hash calldata txId,
        Tip5Hash calldata nameFirst,
        Tip5Hash calldata nameLast,
        address recipient,
        uint256 amount,
        uint256 blockHeight,
        Tip5Hash calldata asOf,
        uint256 depositNonce,
        bytes[] calldata ethSigs
    ) external {
        require(ethSigs.length >= THRESHOLD, "Insufficient Ethereum signatures");
        require(_isValidTip5(txId), "Invalid txId");
        require(_isValidTip5(nameFirst), "Invalid nameFirst");
        require(_isValidTip5(nameLast), "Invalid nameLast");
        require(_isValidTip5(asOf), "Invalid asOf");
        require(!_isZeroTip5(asOf), "Invalid as-of hash");
        require(amount > 0, "Amount must be positive");
        require(recipient != address(0), "Invalid recipient");

        bytes32 txIdHash = _hashTip5(txId);

        require(!processedDeposits[txIdHash], "Deposit already processed");
        require(depositNonce > lastDepositNonce, "Nonce must be strictly greater");

        require(
            _verifySignatures(
                _computeDepositHash(txId, nameFirst, nameLast, recipient, amount, blockHeight, asOf, depositNonce),
                ethSigs
            ),
            "Invalid Ethereum signatures"
        );

        processedDeposits[txIdHash] = true;
        lastDepositNonce = depositNonce;
        nock.mint(recipient, amount);

        emit DepositProcessed(txIdHash, _hashTip5(nameFirst), recipient, txId, nameFirst, nameLast, amount, blockHeight, asOf, depositNonce);
    }


    /**
     * @dev Update bridge node address
     * @param index Index of bridge node (0-4)
     * @param newNode New bridge node address
     */
    function updateBridgeNode(
        uint256 index,
        address newNode
    ) external onlyOwner {
        require(index < 5, "Invalid bridge node index");
        require(newNode != address(0), "Invalid bridge node address");

        // Check that newNode doesn't duplicate any existing node
        for (uint256 i = 0; i < 5; i++) {
            if (i != index) {
                require(bridgeNodes[i] != newNode, "Duplicate bridge node address");
            }
        }

        address oldNode = bridgeNodes[index];
        bridgeNodes[index] = newNode;

        emit BridgeNodeUpdated(index, oldNode, newNode);
    }

    /**
     * @dev Toggle withdrawals (Base -> Nockchain) on/off.
     * When disabled, Nock.burn() reverts atomically since it calls notifyBurn.
     * @param enabled Whether withdrawals should be enabled
     */
    function setWithdrawalsEnabled(bool enabled) external onlyOwner {
        withdrawalsEnabled = enabled;
        emit WithdrawalsToggled(enabled);
    }

    /**
     * @dev Notify of burn for withdrawal (called by Nock contract).
     * Reverts if withdrawals disabled, causing the entire burn tx to revert.
     */
    function notifyBurn() external view {
        require(withdrawalsEnabled, "Withdrawals are disabled");
        require(
            msg.sender == address(nock),
            "Only Nock contract can notify burns"
        );
    }

    function _isValidTip5(Tip5Hash calldata h) internal pure returns (bool) {
        uint64 PRIME = 0xffffffff00000001;
        return h.limbs[0] < PRIME && h.limbs[1] < PRIME && h.limbs[2] < PRIME && h.limbs[3] < PRIME && h.limbs[4] < PRIME;
    }

    function _isZeroTip5(Tip5Hash calldata h) internal pure returns (bool) {
        return h.limbs[0] == 0 && h.limbs[1] == 0 && h.limbs[2] == 0 && h.limbs[3] == 0 && h.limbs[4] == 0;
    }

    function _encodeTip5(Tip5Hash calldata h) internal pure returns (bytes memory) {
        return abi.encodePacked(h.limbs[0], h.limbs[1], h.limbs[2], h.limbs[3], h.limbs[4]);
    }

    function _hashTip5(Tip5Hash calldata h) internal pure returns (bytes32) {
        return keccak256(_encodeTip5(h));
    }

    function _computeDepositHash(
        Tip5Hash calldata txId,
        Tip5Hash calldata nameFirst,
        Tip5Hash calldata nameLast,
        address recipient,
        uint256 amount,
        uint256 blockHeight,
        Tip5Hash calldata asOf,
        uint256 depositNonce
    ) internal pure returns (bytes32) {
        return keccak256(abi.encodePacked(
            _encodeTip5(txId),
            _encodeTip5(nameFirst),
            _encodeTip5(nameLast),
            recipient, amount, blockHeight,
            _encodeTip5(asOf),
            depositNonce
        ));
    }

    function _verifySignatures(bytes32 messageHash, bytes[] calldata sigs) internal view returns (bool) {
        bytes32 ethSignedMessageHash = keccak256(
            abi.encodePacked("\x19Ethereum Signed Message:\n32", messageHash)
        );

        uint256 validSigs = 0;
        bool[5] memory seen;
        for (uint256 i = 0; i < sigs.length; i++) {
            address signer = recoverSigner(ethSignedMessageHash, sigs[i]);
            (bool isNode, uint256 index) = findNodeIndex(signer);
            if (isNode && !seen[index]) {
                seen[index] = true;
                validSigs++;
            }
        }

        return validSigs >= THRESHOLD;
    }

    /**
     * @dev Check if address is a valid bridge node
     */
    function isBridgeNode(address node) internal view returns (bool) {
        for (uint256 i = 0; i < 5; i++) {
            if (bridgeNodes[i] == node) {
                return true;
            }
        }
        return false;
    }

    function findNodeIndex(
        address signer
    ) internal view returns (bool, uint256) {
        for (uint256 i = 0; i < 5; i++) {
            if (bridgeNodes[i] == signer) {
                return (true, i);
            }
        }
        return (false, 0);
    }

    /**
     * @dev Recover signer from signature
     */
    function recoverSigner(
        bytes32 messageHash,
        bytes memory signature
    ) internal pure returns (address) {
        require(signature.length == 65, "Invalid signature length");

        bytes32 r;
        bytes32 s;
        uint8 v;

        assembly {
            r := mload(add(signature, 32))
            s := mload(add(signature, 64))
            v := byte(0, mload(add(signature, 96)))
        }

        require(
            uint256(s) > 0 && uint256(s) <= SECP256K1_HALF_N,
            "Invalid signature s value"
        );
        require(v == 27 || v == 28, "Invalid signature v value");

        address signer = ecrecover(messageHash, v, r, s);
        require(signer != address(0), "Invalid signature");
        return signer;
    }

    /**
     * @dev Authorize upgrade (only owner)
     */
    function _authorizeUpgrade(
        address newImplementation
    ) internal override onlyOwner {}

    /**
     * @dev Upgrade the implementation (only owner, through proxy)
     * This is a convenience function that calls upgradeToAndCall with empty data
     */
    function upgradeTo(
        address newImplementation
    ) public onlyOwner {
        upgradeToAndCall(newImplementation, new bytes(0));
    }
}
