const { contractWith, stateDef, constantDef } = require('../../common/contract-builder')

module.exports = contractWith([stateDef(10), constantDef(10)].join('\n'))
