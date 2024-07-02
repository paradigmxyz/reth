const { funcWith } = require('../../common/contract-builder')

module.exports = funcWith(`require(!has(role, account));
          role.bearer[account] = true;
          role.bearer[account] = true;`)
