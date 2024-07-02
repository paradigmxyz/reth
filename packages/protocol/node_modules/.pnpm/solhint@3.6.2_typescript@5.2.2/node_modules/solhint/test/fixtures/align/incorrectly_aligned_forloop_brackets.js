const { funcWith } = require('../../common/contract-builder')

module.exports = funcWith(`
    for (uint i = 0; i < a; i += 1) 
    {
      continue;
    }
`)
