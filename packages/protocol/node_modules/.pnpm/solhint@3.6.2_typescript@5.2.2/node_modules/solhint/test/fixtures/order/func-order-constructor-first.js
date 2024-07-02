const { contractWith } = require('../../common/contract-builder')

module.exports = contractWith(`
                constructor() public {}
                function () public payable {}
            `)
