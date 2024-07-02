const { contractWith } = require('../../common/contract-builder')

module.exports = contractWith('function a() ownable() public payable {}')
