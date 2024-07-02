import {
  ByteVectorType,
  ContainerType,
  ListCompositeType,
  UintBigintType,
  UintNumberType,
} from '@chainsafe/ssz'

import { MAX_WITHDRAWALS_PER_PAYLOAD } from './constants'

export const UintNum64 = new UintNumberType(8)
export const UintBigInt64 = new UintBigintType(8)
export const Bytes20 = new ByteVectorType(20)

export const Withdrawal = new ContainerType(
  {
    index: UintBigInt64,
    validatorIndex: UintBigInt64,
    address: Bytes20,
    amount: UintBigInt64,
  },
  { typeName: 'Withdrawal', jsonCase: 'eth2' }
)
export const Withdrawals = new ListCompositeType(Withdrawal, MAX_WITHDRAWALS_PER_PAYLOAD)
