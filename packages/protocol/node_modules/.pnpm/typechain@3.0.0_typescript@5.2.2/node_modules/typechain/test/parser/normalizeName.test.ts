import { expect } from 'chai'

import { normalizeName } from '../../src/parser/normalizeName'

describe('name normalizer', () => {
  it('should work', () => {
    expect(normalizeName('DsToken')).to.be.eq('DsToken')
    expect(normalizeName('test')).to.be.eq('Test')
    expect(normalizeName('ds-token')).to.be.eq('DsToken')
    expect(normalizeName('ds_token')).to.be.eq('DsToken')
    expect(normalizeName('ds token')).to.be.eq('DsToken')
    expect(normalizeName('name.abi')).to.be.eq('NameAbi')
    expect(normalizeName('1234name.abi')).to.be.eq('NameAbi')
  })
})
