const { funcWith } = require('../../common/contract-builder')

module.exports = funcWith(`require(!has(role, account), "Roles: account already has role");
          role.bearer[account] = true;
          role.bearer[account] = true;`)
