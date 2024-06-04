# 选举合约接口说明

## 创建提案

    function createProposal(
        bytes32 name,
        uint256 lastBlocks,
        bytes memory candidate,
        uint8 action
    )

    name: 提案名称
    lastBlocks： 提案的生命时长
    candidate:被选举的候选人
    action:  0 裁撤候选人， 1 添加候选人

## 投票

    function vote(bytes32 name, uint8 action)
    
    name: 提案名称
    action： 投票动作 0 反对，1 支持

## 查询提案

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
    
    proposal_name：提案名称
    返回值：status（0 超出生命周期 1 被接收 2 被拒绝 3 被投票中） 提案状态，total_power_against 总反对数， total_power_for 总支持数

## 查询验证节点
    function allValidators(
        uint block_number
    ) public view returns (bytes32[] memory)

    block_number:查询区块号对应的验证节点
    返回值：一组验证节点，其中每两个组成一个节点标识