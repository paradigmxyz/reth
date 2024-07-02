const bn128 = require('./lib/index.asm.js')

const ec_add = bn128.cwrap('ec_add', 'string', ['string'])
const ec_mul = bn128.cwrap('ec_mul', 'string', ['string'])
const ec_pairing = bn128.cwrap('ec_pairing', 'string', ['string'])

function bn128add (input) {
  return Buffer.from(ec_add(input.toString('hex')), 'hex')
}

function bn128mul (input) {
  return Buffer.from(ec_mul(input.toString('hex')), 'hex')
}

function bn128pairing (input) {
  return Buffer.from(ec_pairing(input.toString('hex')), 'hex')
}

module.exports = {
  add: bn128add,
  mul: bn128mul,
  pairing: bn128pairing
}
