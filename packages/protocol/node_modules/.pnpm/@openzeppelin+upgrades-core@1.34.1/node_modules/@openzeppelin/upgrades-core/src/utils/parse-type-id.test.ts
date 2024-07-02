import test from 'ava';

import { parseTypeId } from './parse-type-id';

const fixtures = [
  't_uint256',
  't_enum(MyEnum)',
  't_struct(MyComplexStruct)storage',
  't_array(t_uint256)3_storage',
  't_mapping(t_uint256,t_uint256)',
  't_mapping(unknown,t_uint256)',
  't_mapping(t_uint256,t_array(t_bool)dyn_storage)',
  't_mapping(t_uint256,t_mapping(t_string_memory_ptr,t_address))',
  't_function_internal_nonpayable(t_uint256)returns()',
  't_array(t_function_internal_nonpayable(t_uint256)returns(t_address))10_storage',
];

for (const f of fixtures) {
  test(f, t => {
    t.snapshot(parseTypeId(f));
  });
}
