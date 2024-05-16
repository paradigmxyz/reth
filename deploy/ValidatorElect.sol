// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

contract VotingEvents {
    event NewProposal(bytes32 _name, uint256 _lastBlocks);
    event ProposalDiscarded(bytes32 _name);
    event ProposalAccepted(bytes32 _name);
    event ProposalRejected(bytes32 _name);
    event Voted(
        bytes32 _name,
        address indexed _voter,
        uint256 _votes,
        uint8 _choose
    );
}

contract ValidatorElect is VotingEvents {
    uint8 constant PROPOSALS_THRESHOLD = 3;
    uint8 constant VALIDATORS_MAX = 20;
    uint128 constant TOTAL_SUPPLY = 100e6;
    uint128 constant VOTES_THRESHOLD = TOTAL_SUPPLY / 2;

    PeerKey[] mValidators; // slot 0
    Proposal[PROPOSALS_THRESHOLD] public mProposals;

    struct PeerKey {
        bytes32 Half1;
        bytes32 Half2;
    }

    struct Proposal {
        bytes32 name; // short name (up to 32 bytes)
        uint256 deadline; // expiring of the proposal
        uint256 yesCount; // number of possitive votes
        uint256 noCount; // number of negative votes
        ProposalStatus status;
        PeerKey candidate;
        ValidatorAction action;
    }

    mapping(address => mapping(bytes32 => VoteAction)) mVotes;

    /**
     * Represents the proposal's status.
     */
    enum ProposalStatus {
        /**
         * Time-to-live limit reached.
         */
        Discarded,
        /**
         * Proposal accepted.
         */
        Accepted,
        /**
         * Proposal rejected.
         */
        Rejected,
        /**
         * Voting is in progress, the time limit and the required
         * number of votes "for" and "against" have not yet been riched.
         */
        Indefinite
    }

    enum ValidatorAction {
        Reduce,
        Add
    }

    enum VoteAction {
        Oppose,
        Support
    }

    constructor() {
        mValidators.push(
            PeerKey({
                Half1: 0x23fc99dc5a9411b1f74425cf82d38393f9f0dfa63360848886514eb64f8d61c9,
                Half2: 0x9a47b52df97afbad20bdbd781086a9e9e228a4d61177d85e28f8cdf5c6ae7738
            })
        );
        mValidators.push(
            PeerKey({
                Half1: 0x3dafa30585c0db38057f633fd5fc1661648cf9eb9442534bbeee25e998342ddb,
                Half2: 0xf3f8db23956ce65cc97e0099103d97576bb8ee030d33e099363de4ddf88a8b44
            })
        );
        mValidators.push(
            PeerKey({
                Half1: 0x6202a42963acfe9ed6bdb11e1cacdb61fcb5fc0a967972acf675286d3a546276,
                Half2: 0xc633ad0911a8bb7ecb2ad5dded80059f013187a8bc69e5bc6eba18410d8b4a91
            })
        );
        mValidators.push(
            PeerKey({
                Half1: 0xfecf8e7cf8ac4c9eef1bbc8c7c10ee72999e155a3d6c51b6026a55e633c0f5d2,
                Half2: 0x50d5ad437b3552f8f11144f9a2029736e46d63a2d3551b156aac9bff9a6e22bd
            })
        );
        // mValidators.push(
        //     PeerKey({
        //         Half1: 0x75b4f60228da49149a793f3e0cca2a280159c59d8ac0c5a4c55a73f9cea225e5,
        //         Half2: 0xfc72166353550424a778bb9e15c47ed89a825e0ea61867ccecffbbf5ed963260
        //     })
        // );
    }

    function allValidators() public view returns (bytes32[] memory) {
        uint arrayLength = mValidators.length;
        bytes32[] memory ret = new bytes32[](arrayLength * 2);
        for (uint i = 0; i < arrayLength; i++) {
            ret[i * 2] = mValidators[i].Half1;
            ret[i * 2 + 1] = mValidators[i].Half2;
        }
        return ret;
    }

    function _addValidator(PeerKey memory p) private {
        uint arrayLength = mValidators.length;
        mValidators[arrayLength] = PeerKey({Half1: p.Half1, Half2: p.Half2});
    }

    function _delValidator(PeerKey memory p) private {
        bool flag = false;
        uint arrayLength = mValidators.length;
        for (uint i = 0; i < arrayLength; i++) {
            PeerKey memory p1 = mValidators[i];
            if (_comparePeer(p1, p)) {
                flag = true;
            }
            if (flag && i + 1 < arrayLength) {
                mValidators[i].Half1 = mValidators[i + 1].Half1;
                mValidators[i].Half2 = mValidators[i + 1].Half2;
            }
        }
        mValidators.pop();
    }

    function _comparePeer(
        PeerKey memory a,
        PeerKey memory b
    ) private pure returns (bool) {
        return a.Half1 == b.Half1 && a.Half2 == b.Half2;
    }

    function _containPeer(PeerKey memory p) private view returns (bool) {
        uint arrayLength = mValidators.length;
        for (uint i = 0; i < arrayLength; i++) {
            PeerKey memory p1 = mValidators[i];
            if (_comparePeer(p1, p)) {
                return true;
            }
        }
        return false;
    }

    function _bytesToPeer(
        bytes memory data
    ) private pure returns (PeerKey memory peer) {
        uint256 dataNb = data.length / 32;
        require(data.length % 32 == 0 && dataNb == 2);
        bytes32[] memory dataList = new bytes32[](dataNb);
        uint256 index = 0;
        for (uint256 i = 32; i <= data.length; i = i + 32) {
            bytes32 temp;
            assembly {
                temp := mload(add(data, i))
            }
            // Add extracted 32 bytes to list
            dataList[index] = temp;
            index++;
        }

        peer = PeerKey({Half1: dataList[0], Half2: dataList[1]});
        return peer;
    }

    function _concat(
        bytes32 b1,
        bytes32 b2
    ) private pure returns (bytes memory) {
        bytes memory result = new bytes(64);
        assembly {
            mstore(add(result, 32), b1)
            mstore(add(result, 64), b2)
        }
        return result;
    }

    function _peerToBytes(
        PeerKey memory p
    ) private pure returns (bytes memory) {
        return _concat(p.Half1, p.Half2);
    }

    function _getAddressFromPublicKey(
        bytes memory publicKey
    ) public pure returns (address addr) {
        bytes32 publicKeyHash = keccak256(publicKey);
        // NOTE: I take the last 40 characters from the end of publicKeyHash variable
        //       and convert them into address.
        addr = address(uint160(uint256(publicKeyHash)));
        return addr;
    }

    function _peerToAddress(PeerKey memory p) public pure returns (address) {
        bytes memory data = _peerToBytes(p);
        return _getAddressFromPublicKey(data);
    }

    function _findFreePlace(bytes32 proposal_name) private returns (uint8) {
        uint8 finished = PROPOSALS_THRESHOLD;
        uint8 oldest_discarded = PROPOSALS_THRESHOLD;

        for (uint8 i = 0; i < PROPOSALS_THRESHOLD; i++) {
            Proposal storage proposal = mProposals[i];
            _checkTTL(proposal);
            if (proposal.status == ProposalStatus.Indefinite) {
                // The new proposal must not coincide with any actual (with
                // `Indefinite` status) proposal in the queue:
                require(
                    proposal_name != proposal.name,
                    "The proposal is already in the queue."
                );
                continue;
            }
            if (proposal.status != ProposalStatus.Discarded) {
                // Is `Accepted` or `Rejected
                finished = i;
                continue;
            }
            // is `Discarded`
            if (
                oldest_discarded == PROPOSALS_THRESHOLD ||
                proposal.deadline < mProposals[oldest_discarded].deadline
            ) {
                oldest_discarded = i;
            }
        }

        if (finished != PROPOSALS_THRESHOLD) {
            return finished;
        }
        return oldest_discarded;
    }

    function _lookupProposal(
        bytes32 proposal_name
    ) private view returns (uint8) {
        for (uint8 i = 0; i < PROPOSALS_THRESHOLD; i++) {
            if (mProposals[i].name == proposal_name) {
                return i;
            }
        }
        return PROPOSALS_THRESHOLD;
    }

    function _checkTTL(
        Proposal storage proposal
    ) private _IsIndefinite(proposal) {
        if (_TTLExceeded(proposal)) {
            proposal.status = ProposalStatus.Discarded;
            emit ProposalDiscarded(proposal.name);
        }
    }

    function _TTLExceeded(
        Proposal storage proposal
    ) private view returns (bool) {
        return block.number > proposal.deadline;
    }

    modifier _IsIndefinite(Proposal storage proposal) {
        if (proposal.status == ProposalStatus.Indefinite) {
            _;
        }
    }

    modifier _IsValidator(address sender) {
        bool is_validator = false;
        uint arrayLength = mValidators.length;
        for (uint i = 0; i < arrayLength; i++) {
            PeerKey memory p = mValidators[i];
            address addr = _peerToAddress(p);
            if (sender == msg.sender) {
                is_validator = true;
            }
        }
        require(is_validator, "Sender must validator");
        _;
    }

    function _updateStatus(
        Proposal storage proposal
    ) private _IsIndefinite(proposal) {
        uint256 yes = proposal.yesCount;
        uint256 no = proposal.noCount;
        uint arrayLength = mValidators.length;

        if ((yes + no) > (arrayLength / 2)) {
            if (no > yes) {
                proposal.status = ProposalStatus.Rejected;
                emit ProposalRejected(proposal.name);
            } else {
                proposal.status = ProposalStatus.Accepted;

                PeerKey memory peer = proposal.candidate;
                if (proposal.action == ValidatorAction.Add) {
                    _addValidator(peer);
                } else {
                    _delValidator(peer);
                }
                emit ProposalAccepted(proposal.name);
            }
        }
    }

    function createProposal(
        bytes32 _name,
        uint256 lastBlocks,
        bytes memory candidate,
        uint8 action
    ) public _IsValidator(msg.sender) returns (uint256 indexOfProposal) {
        PeerKey memory peer = _bytesToPeer(candidate);

        indexOfProposal = _findFreePlace(_name);
        require(
            indexOfProposal != PROPOSALS_THRESHOLD,
            "The limit of active proposals has been reached"
        );

        ValidatorAction vaction = ValidatorAction(action);
        if (vaction == ValidatorAction.Reduce) {
            require(_containPeer(peer), "must contain candidate");
        } else {
            require(!_containPeer(peer), "must not contain candidate");
        }

        mProposals[indexOfProposal] = Proposal({
            name: _name,
            deadline: block.number + lastBlocks,
            yesCount: 0,
            noCount: 0,
            status: ProposalStatus.Indefinite,
            candidate: peer,
            action: vaction
        });
        emit NewProposal(_name, lastBlocks);
    }

    function vote(bytes32 _name, uint8 action) public _IsValidator(msg.sender) {
        uint8 indexOfProposal = _lookupProposal(_name);
        require(indexOfProposal != PROPOSALS_THRESHOLD, "Unknown proposal");

        Proposal storage proposal = mProposals[indexOfProposal];
        _checkTTL(proposal);
        require(
            proposal.status == ProposalStatus.Indefinite,
            "Voting finished"
        );

        //voteToken.transferFrom(msg.sender, address(this), votes);
        //lockedTokens[indexOfProposal][msg.sender] += votes;

        VoteAction choose = VoteAction(action);
        address voter = msg.sender;
        VoteAction last = mVotes[voter][_name];
        if (choose == last) {
            return;
        }
        mVotes[voter][_name] = choose;

        if (choose == VoteAction.Support) {
            mProposals[indexOfProposal].yesCount += 1;
        } else {
            mProposals[indexOfProposal].noCount += 1;
        }
        emit Voted(_name, msg.sender, 1, action);

        _updateStatus(proposal);
    }

    function allProposals()
        public
        view
        returns (bytes32[PROPOSALS_THRESHOLD] memory all_proposals)
    {
        uint8 size = 0;

        for (uint8 i = 0; i < PROPOSALS_THRESHOLD; i++) {
            all_proposals[size++] = mProposals[i].name;
        }
    }

    function activeProposals()
        public
        view
        returns (bytes32[] memory active_proposals)
    {
        uint8 size = 0;
        bytes32[] memory _active_proposals = new bytes32[](PROPOSALS_THRESHOLD);

        for (uint8 i = 0; i < PROPOSALS_THRESHOLD; i++) {
            Proposal storage proposal = mProposals[i];
            if (
                proposal.status == ProposalStatus.Indefinite &&
                !_TTLExceeded(proposal)
            ) {
                _active_proposals[size++] = proposal.name;
            }
        }
        if (size > 0) {
            active_proposals = new bytes32[](size);
            for (uint i = 0; i < size; i++) {
                active_proposals[i] = _active_proposals[i];
            }
        }
    }

    function proposalInfo(
        bytes32 proposal_name
    )
        public
        view
        returns (
            ProposalStatus status,
            uint256 total_power_against,
            uint256 total_power_for
        )
    {
        uint8 proposal_index = _lookupProposal(proposal_name);
        require(proposal_index != PROPOSALS_THRESHOLD, "Unknown proposal");

        Proposal storage proposal = mProposals[proposal_index];

        total_power_against = proposal.noCount;
        total_power_for = proposal.yesCount;

        if (proposal.status != ProposalStatus.Indefinite) {
            status = proposal.status;
        } else if (_TTLExceeded(proposal)) {
            status = ProposalStatus.Discarded;
        } else {
            status = ProposalStatus.Indefinite;
        }
    }
}
