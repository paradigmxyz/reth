const createModule = require('../../src/mcl_c.js')
const setupFactory = require('../../src/mcl.js')
const crypto = window.crypto || window.msCrypto

const getRandomValues = x => crypto.getRandomValues(x)
const mcl = setupFactory(createModule, getRandomValues)

module.exports = mcl

