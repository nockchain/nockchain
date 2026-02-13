// SPDX-License-Identifier: MIT
pragma solidity ^0.8.19;

import "@openzeppelin/contracts/token/ERC20/ERC20.sol";
import "@openzeppelin/contracts/access/Ownable.sol";

interface IMessageInbox {
    function notifyBurn() external;
}

/**
 * @title Nock
 * @dev ERC-20 token representing wrapped Nockchain assets
 *
 * This contract is NOT upgradeable to ensure immutability of the token.
 * Users can burn tokens to initiate withdrawals to Nockchain.
 * Only the MessageInbox contract can mint new tokens.
 */
contract Nock is ERC20, Ownable {
    // MessageInbox contract address (only it can mint)
    address public inbox;

    // Events
    event BurnForWithdrawal(
        address indexed burner,
        uint256 amount,
        bytes32 indexed lockRoot
    );

    event InboxUpdated(address indexed oldInbox, address indexed newInbox);

    /**
     * @dev Constructor
     * @param _name Token name
     * @param _symbol Token symbol
     * @param _inbox Address of MessageInbox contract
     */
    constructor(
        string memory _name,
        string memory _symbol,
        address _inbox
    ) ERC20(_name, _symbol) Ownable(msg.sender) {
        inbox = _inbox;
    }

    /**
     * @dev Mint tokens (only MessageInbox can call this)
     * @param to Recipient address
     * @param amount Amount to mint
     */
    function mint(address to, uint256 amount) external {
        require(msg.sender == inbox, "Only inbox can mint");
        require(to != address(0), "Cannot mint to zero address");
        require(amount > 0, "Amount must be positive");

        _mint(to, amount);
    }

    /**
     * @dev Burn tokens to initiate withdrawal to Nockchain
     * @param amount Amount to burn
     * @param lockRoot Lock script root hash for Nockchain note
     */
    function burn(uint256 amount, bytes32 lockRoot) external {
        require(amount > 0, "Amount must be positive");
        require(balanceOf(msg.sender) >= amount, "Insufficient balance");

        _burn(msg.sender, amount);

        emit BurnForWithdrawal(msg.sender, amount, lockRoot);

        // Notify inbox for withdrawal toggle check
        IMessageInbox(inbox).notifyBurn();
    }

    /**
     * @dev Update MessageInbox address (only owner)
     * @param _newInbox New MessageInbox address
     */
    function updateInbox(address _newInbox) external onlyOwner {
        require(_newInbox != address(0), "Invalid inbox address");

        address oldInbox = inbox;
        inbox = _newInbox;

        emit InboxUpdated(oldInbox, _newInbox);
    }

    /**
     * @dev Get token decimals (16 for Nockchain alignment)
     */
    function decimals() public pure override returns (uint8) {
        return 16;
    }

    /**
     * @dev Get total supply cap (no cap for simplicity)
     */
    function cap() public pure returns (uint256) {
        return 0; // No cap
    }
}
