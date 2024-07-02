'use strict'

const CID = require('../src')
const multihashStr = 'QmPZ9gcCEpqKTo6aq61g2nXGUhM4iCL3ewB6LDXZCtioEB'

const cidv0 = new CID(multihashStr)

console.log(cidv0.toBaseEncodedString())

const cidv1 = cidv0.toV1()

console.log(cidv1.toBaseEncodedString())
