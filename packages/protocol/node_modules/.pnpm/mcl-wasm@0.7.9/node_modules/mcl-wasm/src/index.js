const crypto = require('crypto')
const mclCreateModule = require('./mcl_c.js')
const mclSetupFactory = require('./mcl')

const getRandomValues = crypto.randomFillSync
const mcl = mclSetupFactory(mclCreateModule, getRandomValues)

module.exports = mcl
