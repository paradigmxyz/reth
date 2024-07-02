const { contractWith, funcWith } = require('../../common/contract-builder')

module.exports = [
  contractWith(`
                mapping(address => uint) private shares;

                function b() external {
                    uint amount = shares[msg.sender];
                    shares[msg.sender] = 0;
                    msg.sender.transfer(amount);
                }
            `),
  contractWith(`
                mapping(address => uint) private shares;

                function b() external {
                    uint amount = shares[msg.sender];
                    user.test(amount);
                    shares[msg.sender] = 0;
                }
            `),
  funcWith(`
                uint[] shares;
                uint amount = shares[msg.sender];
                msg.sender.transfer(amount);
                shares[msg.sender] = 0;
            `),
]
