import { expect } from 'chai'
import { parseEvmType, UnsignedIntegerType, IntegerType, ArrayType, BytesType } from '../../src/parser/parseEvmType'

describe('parseEvmType function', () => {
  it('parses unsigned integer', () => {
    const parsedType = parseEvmType('uint8')

    expect(parsedType.type).to.be.eq('uinteger')
    expect((parsedType as UnsignedIntegerType).bits).to.be.eq(8)
  })

  it('parses signed integer', () => {
    const parsedType = parseEvmType('int')

    expect(parsedType.type).to.be.eq('integer')
    expect((parsedType as IntegerType).bits).to.be.eq(256)
  })

  it('parses boolean', () => {
    const parsedType = parseEvmType('bool')

    expect(parsedType.type).to.be.eq('boolean')
  })

  it('parses bytes2', () => {
    const parsedType = parseEvmType('bytes2')

    expect(parsedType.type).to.be.eq('bytes')
    expect((parsedType as BytesType).size).to.be.eq(2)
  })

  it('parses bytes', () => {
    const parsedType = parseEvmType('bytes')

    expect(parsedType.type).to.be.eq('dynamic-bytes')
  })

  it('parses arrays', () => {
    const parsedType = parseEvmType('uint[]')

    expect(parsedType.type).to.be.eq('array')
    expect((parsedType as ArrayType).itemType.type).to.be.eq('uinteger')
  })

  it('parses fixed size arrays', () => {
    const parsedType = parseEvmType('uint[8]')

    expect(parsedType.type).to.be.eq('array')
    expect((parsedType as ArrayType).itemType.type).to.be.eq('uinteger')
    expect((parsedType as ArrayType).size).to.be.eq(8)
  })

  it('parses nested arrays', () => {
    const parsedType = parseEvmType('uint16[8][256]')

    expect(parsedType.type).to.be.eq('array')
    expect((parsedType as ArrayType).itemType.type).to.be.eq('array')
    expect((parsedType as ArrayType).size).to.be.eq(256)
    expect(((parsedType as ArrayType).itemType as ArrayType).itemType.type).to.be.eq('uinteger')
    expect(((parsedType as ArrayType).itemType as ArrayType).size).to.be.eq(8)
    expect((((parsedType as ArrayType).itemType as ArrayType).itemType as UnsignedIntegerType).bits).to.be.eq(16)
  })

  it('parses tuples', () => {
    const parsedType = parseEvmType('tuple', [
      {
        name: 'uint256_0',
        type: { type: 'uinteger', bits: 256, originalType: 'uint256' },
      },
      {
        name: 'uint256_1',
        type: { type: 'uinteger', bits: 256, originalType: 'uint256' },
      },
    ])
    expect(parsedType).to.be.deep.eq({
      type: 'tuple',
      components: [
        { name: 'uint256_0', type: { type: 'uinteger', bits: 256, originalType: 'uint256' } },
        { name: 'uint256_1', type: { type: 'uinteger', bits: 256, originalType: 'uint256' } },
      ],
      originalType: 'tuple',
    })
  })

  // Turns out that USUALLY solidity won't leave enums in abis but for some reason they are part of libraries abis
  // This is a test for workaround that forces it to parse as uint8
  // Related issue: https://github.com/ethereum-ts/TypeChain/issues/216
  it('parses enums in libraries', () => {
    const parsedType = parseEvmType('Lib.BOOL', undefined, 'enum Lib.BOOL')

    expect(parsedType.type).to.be.eq('uinteger')
    expect((parsedType as UnsignedIntegerType).bits).to.be.eq(8)
  })
})
