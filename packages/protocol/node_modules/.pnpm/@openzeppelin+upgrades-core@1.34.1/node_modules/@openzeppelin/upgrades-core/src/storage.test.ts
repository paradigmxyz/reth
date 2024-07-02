import _test, { TestFn } from 'ava';
import { ContractDefinition } from 'solidity-ast';
import { findAll, astDereferencer } from 'solidity-ast/utils';
import { artifacts } from 'hardhat';

import { SolcOutput } from './solc-api';
import { getStorageUpgradeErrors } from './storage';
import { StorageLayout } from './storage/layout';
import { extractStorageLayout } from './storage/extract';
import { stabilizeTypeIdentifier } from './utils/type-id';
import { stabilizeStorageLayout } from './utils/stabilize-layout';

interface Context {
  extractStorageLayout: (contract: string) => ReturnType<typeof extractStorageLayout>;
}

const test = _test as TestFn<Context>;

test.before(async t => {
  const buildInfo = await artifacts.getBuildInfo('contracts/test/Storage.sol:Storage1');
  if (buildInfo === undefined) {
    throw new Error('Build info not found');
  }
  const solcOutput: SolcOutput = buildInfo.output;
  const contracts: Record<string, ContractDefinition> = {};
  const storageLayouts: Record<string, StorageLayout> = {};
  for (const def of findAll('ContractDefinition', solcOutput.sources['contracts/test/Storage.sol'].ast)) {
    contracts[def.name] = def;
    // eslint-disable-next-line @typescript-eslint/no-non-null-assertion
    storageLayouts[def.name] = solcOutput.contracts['contracts/test/Storage.sol'][def.name].storageLayout!;
  }
  const deref = astDereferencer(solcOutput);
  t.context.extractStorageLayout = name =>
    extractStorageLayout(contracts[name], dummyDecodeSrc, deref, storageLayouts[name]);
});

const dummyDecodeSrc = () => 'file.sol:1';

test('Storage1', t => {
  const layout = t.context.extractStorageLayout('Storage1');
  t.snapshot(stabilizeStorageLayout(layout));
});

test('Storage2', t => {
  const layout = t.context.extractStorageLayout('Storage2');
  t.snapshot(stabilizeStorageLayout(layout));
});

test('storage upgrade equal', t => {
  const v1 = t.context.extractStorageLayout('StorageUpgrade_Equal_V1');
  const v2 = t.context.extractStorageLayout('StorageUpgrade_Equal_V2');
  const comparison = getStorageUpgradeErrors(v1, v2);
  t.deepEqual(comparison, []);
});

test('storage upgrade append', t => {
  const v1 = t.context.extractStorageLayout('StorageUpgrade_Append_V1');
  const v2 = t.context.extractStorageLayout('StorageUpgrade_Append_V2');
  const comparison = getStorageUpgradeErrors(v1, v2);
  t.deepEqual(comparison, []);
});

test('storage upgrade delete', t => {
  const v1 = t.context.extractStorageLayout('StorageUpgrade_Delete_V1');
  const v2 = t.context.extractStorageLayout('StorageUpgrade_Delete_V2');
  const comparison = getStorageUpgradeErrors(v1, v2);
  t.like(comparison, {
    length: 2,
    0: {
      kind: 'delete',
      original: {
        contract: 'StorageUpgrade_Delete_V1',
        label: 'x1',
        type: {
          id: 't_uint256',
        },
      },
    },
    1: {
      kind: 'layoutchange',
      original: {
        label: 'x2',
        type: {
          id: 't_address',
        },
        slot: '1',
      },
      updated: {
        label: 'x2',
        type: {
          id: 't_address',
        },
        slot: '0',
      },
    },
  });
});

test('storage upgrade replace', t => {
  const v1 = t.context.extractStorageLayout('StorageUpgrade_Replace_V1');
  const v2 = t.context.extractStorageLayout('StorageUpgrade_Replace_V2');
  const comparison = getStorageUpgradeErrors(v1, v2);
  t.like(comparison, {
    length: 1,
    0: {
      kind: 'replace',
      original: {
        contract: 'StorageUpgrade_Replace_V1',
        label: 'x2',
        type: {
          id: 't_uint256',
        },
      },
      updated: {
        contract: 'StorageUpgrade_Replace_V2',
        label: 'renamed',
        type: {
          id: 't_string_storage',
        },
      },
    },
  });
});

test('storage upgrade rename', t => {
  const v1 = t.context.extractStorageLayout('StorageUpgrade_Rename_V1');
  const v2 = t.context.extractStorageLayout('StorageUpgrade_Rename_V2');
  const comparison = getStorageUpgradeErrors(v1, v2);
  t.like(comparison, {
    length: 1,
    0: {
      kind: 'rename',
      original: {
        contract: 'StorageUpgrade_Rename_V1',
        label: 'x2',
        type: {
          id: 't_uint256',
        },
      },
      updated: {
        contract: 'StorageUpgrade_Rename_V2',
        label: 'renamed',
        type: {
          id: 't_uint256',
        },
      },
    },
  });
});

test('storage upgrade rename allowed', t => {
  const v1 = t.context.extractStorageLayout('StorageUpgrade_Rename_V1');
  const v2 = t.context.extractStorageLayout('StorageUpgrade_Rename_V2');
  const comparison = getStorageUpgradeErrors(v1, v2, { unsafeAllowRenames: true });
  t.is(comparison.length, 0);
});

test('storage upgrade with obvious mismatch', t => {
  const v1 = t.context.extractStorageLayout('StorageUpgrade_ObviousMismatch_V1');

  const v2_Bad = t.context.extractStorageLayout('StorageUpgrade_ObviousMismatch_V2_Bad');
  t.like(getStorageUpgradeErrors(v1, v2_Bad), {
    length: 3,
    0: {
      kind: 'typechange',
      change: { kind: 'obvious mismatch' },
      original: { label: 'x1' },
      updated: { label: 'x1' },
    },
    1: {
      kind: 'typechange',
      change: { kind: 'obvious mismatch' },
      original: { label: 's1' },
      updated: { label: 's1' },
    },
    2: {
      kind: 'typechange',
      change: { kind: 'obvious mismatch' },
      original: { label: 'a1' },
      updated: { label: 'a1' },
    },
  });
});

test('storage upgrade with structs', t => {
  const v1 = t.context.extractStorageLayout('StorageUpgrade_Struct_V1');

  const v2_Ok = t.context.extractStorageLayout('StorageUpgrade_Struct_V2_Ok');
  t.deepEqual(getStorageUpgradeErrors(v1, v2_Ok), []);

  const v2_Bad = t.context.extractStorageLayout('StorageUpgrade_Struct_V2_Bad');

  t.like(getStorageUpgradeErrors(v1, v2_Bad), {
    length: 6,
    0: {
      kind: 'typechange',
      change: {
        kind: 'struct members',
        ops: {
          length: 3,
          0: { kind: 'delete' },
        },
      },
      original: { label: 'data1' },
      updated: { label: 'data1' },
    },
    1: {
      kind: 'typechange',
      change: {
        kind: 'struct members',
        ops: {
          length: 1,
          0: { kind: 'append' },
        },
      },
      original: { label: 'data2' },
      updated: { label: 'data2' },
    },
    2: {
      kind: 'typechange',
      change: {
        kind: 'mapping value',
        inner: { kind: 'struct members' },
      },
      original: { label: 'm' },
      updated: { label: 'm' },
    },
    3: {
      kind: 'typechange',
      change: {
        kind: 'array value',
        inner: { kind: 'struct members' },
      },
      original: { label: 'a1' },
      updated: { label: 'a1' },
    },
    4: {
      kind: 'typechange',
      change: {
        kind: 'array value',
        inner: { kind: 'struct members' },
      },
      original: { label: 'a2' },
      updated: { label: 'a2' },
    },
    5: {
      kind: 'typechange',
      change: {
        kind: 'struct members',
        ops: {
          length: 3,
          0: { kind: 'typechange' },
          1: { kind: 'typechange' },
          2: { kind: 'typechange' },
        },
      },
      original: { label: 'data3' },
      updated: { label: 'data3' },
    },
  });
});

test('storage upgrade with missing struct members', t => {
  const v1 = t.context.extractStorageLayout('StorageUpgrade_Struct_V1');
  const v2 = t.context.extractStorageLayout('StorageUpgrade_Struct_V2_Ok');

  const t_struct = Object.keys(v1.types).find(t => stabilizeTypeIdentifier(t) === 't_struct(Struct1)storage');
  if (t_struct === undefined) {
    throw new Error('Struct type not found');
  }

  // Simulate missing struct members
  v1.types[t_struct].members = undefined;

  t.like(getStorageUpgradeErrors(v1, v2), {
    0: {
      kind: 'typechange',
      change: { kind: 'missing members' },
      original: { label: 'data1' },
      updated: { label: 'data1' },
    },
  });

  t.deepEqual(getStorageUpgradeErrors(v1, v2, { unsafeAllowCustomTypes: true }), []);
});

test('storage upgrade with enums', t => {
  const v1 = t.context.extractStorageLayout('StorageUpgrade_Enum_V1');

  const v2_Ok = t.context.extractStorageLayout('StorageUpgrade_Enum_V2_Ok');
  t.deepEqual(getStorageUpgradeErrors(v1, v2_Ok), []);

  const v2_Bad = t.context.extractStorageLayout('StorageUpgrade_Enum_V2_Bad');
  t.like(getStorageUpgradeErrors(v1, v2_Bad), {
    length: 4,
    0: {
      kind: 'typechange',
      change: {
        kind: 'enum members',
        ops: {
          length: 1,
          0: { kind: 'delete' },
        },
      },
      original: { label: 'data1' },
      updated: { label: 'data1' },
    },
    1: {
      kind: 'typechange',
      change: {
        kind: 'enum members',
        ops: {
          length: 1,
          0: { kind: 'replace', original: 'B', updated: 'X' },
        },
      },
      original: { label: 'data2' },
      updated: { label: 'data2' },
    },
    2: {
      kind: 'typechange',
      change: {
        kind: 'enum members',
        ops: {
          length: 1,
          0: { kind: 'insert', updated: 'X' },
        },
      },
      original: { label: 'data3' },
      updated: { label: 'data3' },
    },
    3: {
      kind: 'typechange',
      change: { kind: 'type resize' },
      original: { label: 'data4' },
      updated: { label: 'data4' },
    },
  });
});

test('storage upgrade with missing enum members', t => {
  const v1 = t.context.extractStorageLayout('StorageUpgrade_Enum_V1');
  const v2 = t.context.extractStorageLayout('StorageUpgrade_Enum_V2_Ok');

  const t_enum = Object.keys(v1.types).find(t => stabilizeTypeIdentifier(t) === 't_enum(Enum1)');
  if (t_enum === undefined) {
    throw new Error('Enum type not found');
  }

  // Simulate missing enum members
  v1.types[t_enum].members = undefined;

  t.like(getStorageUpgradeErrors(v1, v2), {
    0: {
      kind: 'typechange',
      change: { kind: 'missing members' },
      original: { label: 'data1' },
      updated: { label: 'data1' },
    },
  });

  t.deepEqual(getStorageUpgradeErrors(v1, v2, { unsafeAllowCustomTypes: true }), []);
});

test('storage upgrade with recursive type', t => {
  const v1 = t.context.extractStorageLayout('StorageUpgrade_Recursive_V1');
  const v2 = t.context.extractStorageLayout('StorageUpgrade_Recursive_V2');
  const e = t.throws(() => getStorageUpgradeErrors(v1, v2));
  t.true(e?.message.includes('Recursion found'));
});

test('storage upgrade with contract type', t => {
  const v1 = t.context.extractStorageLayout('StorageUpgrade_Contract_V1');
  const v2 = t.context.extractStorageLayout('StorageUpgrade_Contract_V2');
  t.deepEqual(getStorageUpgradeErrors(v1, v2), []);
});

test('storage upgrade with arrays', t => {
  const v1 = t.context.extractStorageLayout('StorageUpgrade_Array_V1');

  const v2_Ok = t.context.extractStorageLayout('StorageUpgrade_Array_V2_Ok');
  t.deepEqual(getStorageUpgradeErrors(v1, v2_Ok), []);

  const v2_Bad = t.context.extractStorageLayout('StorageUpgrade_Array_V2_Bad');
  t.like(getStorageUpgradeErrors(v1, v2_Bad), {
    length: 5,
    0: {
      kind: 'typechange',
      change: { kind: 'array shrink' },
      original: { label: 'x1' },
      updated: { label: 'x1' },
    },
    1: {
      kind: 'typechange',
      change: { kind: 'array grow' },
      original: { label: 'x2' },
      updated: { label: 'x2' },
    },
    2: {
      kind: 'typechange',
      change: { kind: 'array dynamic' },
      original: { label: 'x3' },
      updated: { label: 'x3' },
    },
    3: {
      kind: 'typechange',
      change: { kind: 'array dynamic' },
      original: { label: 'x4' },
      updated: { label: 'x4' },
    },
    4: {
      kind: 'typechange',
      change: {
        kind: 'mapping value',
        inner: { kind: 'array shrink' },
      },
      original: { label: 'm' },
      updated: { label: 'm' },
    },
  });
});

test('storage upgrade with mappings', t => {
  const v1 = t.context.extractStorageLayout('StorageUpgrade_Mapping_V1');

  const v2_Ok = t.context.extractStorageLayout('StorageUpgrade_Mapping_V2_Ok');
  t.deepEqual(getStorageUpgradeErrors(v1, v2_Ok), []);

  const v2_Bad = t.context.extractStorageLayout('StorageUpgrade_Mapping_V2_Bad');
  t.like(getStorageUpgradeErrors(v1, v2_Bad), {
    length: 2,
    0: {
      kind: 'typechange',
      change: {
        kind: 'mapping value',
        inner: { kind: 'obvious mismatch' },
      },
      original: { label: 'm1' },
      updated: { label: 'm1' },
    },
    1: {
      kind: 'typechange',
      change: { kind: 'mapping key' },
      original: { label: 'm2' },
      updated: { label: 'm2' },
    },
  });
});

test('storage upgrade with enum key in mapping', t => {
  const v1 = t.context.extractStorageLayout('StorageUpgrade_MappingEnumKey_V1');

  const v2_Ok = t.context.extractStorageLayout('StorageUpgrade_MappingEnumKey_V2_Ok');
  t.deepEqual(getStorageUpgradeErrors(v1, v2_Ok), []);

  const v2_Bad = t.context.extractStorageLayout('StorageUpgrade_MappingEnumKey_V2_Bad');
  t.like(getStorageUpgradeErrors(v1, v2_Bad), {
    length: 3,
    0: {
      kind: 'typechange',
      change: {
        kind: 'mapping key',
        inner: { kind: 'enum members' },
      },
      original: { label: 'm2' },
      updated: { label: 'm2' },
    },
    1: {
      kind: 'typechange',
      change: {
        kind: 'mapping key',
        inner: { kind: 'obvious mismatch' },
      },
      original: { label: 'm3' },
      updated: { label: 'm3' },
    },
    2: {
      kind: 'typechange',
      change: {
        kind: 'mapping key',
        inner: { kind: 'obvious mismatch' },
      },
      original: { label: 'm4' },
      updated: { label: 'm4' },
    },
  });
});

test('storage upgrade with embedded enum inside struct type', t => {
  const v1 = t.context.extractStorageLayout('StorageUpgrade_StructEnum_V2');
  const v2 = t.context.extractStorageLayout('StorageUpgrade_StructEnum_V2');
  t.deepEqual(getStorageUpgradeErrors(v1, v2), []);
});

test('storage upgrade with gap', t => {
  const v1 = t.context.extractStorageLayout('StorageUpgrade_Gap_V1');
  const v2_Ok = t.context.extractStorageLayout('StorageUpgrade_Gap_V2_Ok');

  const v2_Bad1 = t.context.extractStorageLayout('StorageUpgrade_Gap_V2_Bad1');
  const v2_Bad2 = t.context.extractStorageLayout('StorageUpgrade_Gap_V2_Bad2');
  const v2_Bad3 = t.context.extractStorageLayout('StorageUpgrade_Gap_V2_Bad3');
  const v2_Bad4 = t.context.extractStorageLayout('StorageUpgrade_Gap_V2_Bad4');
  const v2_Bad5 = t.context.extractStorageLayout('StorageUpgrade_Gap_V2_Bad5');

  t.deepEqual(getStorageUpgradeErrors(v1, v2_Ok), []);

  t.like(getStorageUpgradeErrors(v1, v2_Bad1), {
    length: 2,
    0: {
      kind: 'layoutchange',
      change: {
        slot: {
          from: '2',
          to: '3',
        },
      },
      original: { label: '__gap' },
      updated: { label: '__gap' },
    },
    1: {
      kind: 'layoutchange',
      change: {
        slot: {
          from: '50',
          to: '51',
        },
      },
      original: { label: 'z' },
      updated: { label: 'z' },
    },
  });

  t.like(getStorageUpgradeErrors(v1, v2_Bad2), {
    length: 2,
    0: {
      kind: 'delete',
      original: { label: 'b' },
    },
    1: {
      kind: 'typechange',
      change: { kind: 'array grow' },
    },
  });

  t.like(getStorageUpgradeErrors(v1, v2_Bad3), {
    length: 2,
    0: {
      kind: 'typechange',
      change: { kind: 'array shrink' },
    },
    1: {
      kind: 'layoutchange',
      change: {
        slot: {
          from: '50',
          to: '49',
        },
      },
      original: { label: 'z' },
      updated: { label: 'z' },
    },
  });

  t.like(getStorageUpgradeErrors(v1, v2_Bad4), {
    length: 2,
    0: {
      kind: 'typechange',
      change: { kind: 'array shrink' },
    },
    1: {
      kind: 'layoutchange',
      change: {
        slot: {
          from: '50',
          to: '49',
        },
      },
      original: { label: 'z' },
      updated: { label: 'z' },
    },
  });

  t.like(getStorageUpgradeErrors(v1, v2_Bad5), {
    length: 2,
    0: {
      kind: 'typechange',
      change: { kind: 'array shrink' },
      original: { label: '__gap' },
      updated: { label: '__gap' },
    },
    1: {
      kind: 'layoutchange',
      change: {
        slot: {
          from: '50',
          to: '51',
        },
      },
      original: { label: 'z' },
      updated: { label: 'z' },
    },
  });
});

test('storage upgrade with bytes32 gap', t => {
  const v1 = t.context.extractStorageLayout('StorageUpgrade_Bytes32Gap_V1');
  const v2_Ok = t.context.extractStorageLayout('StorageUpgrade_Bytes32Gap_V2_Ok');

  const v2_Bad1 = t.context.extractStorageLayout('StorageUpgrade_Bytes32Gap_V2_Bad1');
  const v2_Bad2 = t.context.extractStorageLayout('StorageUpgrade_Bytes32Gap_V2_Bad2');
  const v2_Bad3 = t.context.extractStorageLayout('StorageUpgrade_Bytes32Gap_V2_Bad3');
  const v2_Bad4 = t.context.extractStorageLayout('StorageUpgrade_Bytes32Gap_V2_Bad4');
  const v2_Bad5 = t.context.extractStorageLayout('StorageUpgrade_Bytes32Gap_V2_Bad5');

  t.deepEqual(getStorageUpgradeErrors(v1, v2_Ok), []);

  t.like(getStorageUpgradeErrors(v1, v2_Bad1), {
    length: 2,
    0: {
      kind: 'layoutchange',
      change: {
        slot: {
          from: '2',
          to: '3',
        },
      },
      original: { label: '__gap' },
      updated: { label: '__gap' },
    },
    1: {
      kind: 'layoutchange',
      change: {
        slot: {
          from: '50',
          to: '51',
        },
      },
      original: { label: 'z' },
      updated: { label: 'z' },
    },
  });

  t.like(getStorageUpgradeErrors(v1, v2_Bad2), {
    length: 2,
    0: {
      kind: 'delete',
      original: { label: 'b' },
    },
    1: {
      kind: 'typechange',
      change: { kind: 'array grow' },
    },
  });

  t.like(getStorageUpgradeErrors(v1, v2_Bad3), {
    length: 2,
    0: {
      kind: 'typechange',
      change: { kind: 'array shrink' },
    },
    1: {
      kind: 'layoutchange',
      change: {
        slot: {
          from: '50',
          to: '49',
        },
      },
      original: { label: 'z' },
      updated: { label: 'z' },
    },
  });

  t.like(getStorageUpgradeErrors(v1, v2_Bad4), {
    length: 2,
    0: {
      kind: 'typechange',
      change: { kind: 'array shrink' },
    },
    1: {
      kind: 'layoutchange',
      change: {
        slot: {
          from: '50',
          to: '49',
        },
      },
      original: { label: 'z' },
      updated: { label: 'z' },
    },
  });

  t.like(getStorageUpgradeErrors(v1, v2_Bad5), {
    length: 2,
    0: {
      kind: 'typechange',
      change: { kind: 'array shrink' },
      original: { label: '__gap' },
      updated: { label: '__gap' },
    },
    1: {
      kind: 'layoutchange',
      change: {
        slot: {
          from: '50',
          to: '51',
        },
      },
      original: { label: 'z' },
      updated: { label: 'z' },
    },
  });
});

test('storage upgrade with multiple items consuming a gap', t => {
  const v1 = t.context.extractStorageLayout('StorageUpgrade_MultiConsumeGap_V1');
  const v2_Ok = t.context.extractStorageLayout('StorageUpgrade_MultiConsumeGap_V2_Ok');

  t.deepEqual(getStorageUpgradeErrors(v1, v2_Ok), []);
});

test('storage upgrade that consumes gap and inherited gap', t => {
  const v1a = t.context.extractStorageLayout('StorageUpgrade_EndGap_V1a');
  const v1b = t.context.extractStorageLayout('StorageUpgrade_EndGap_V1b');
  const v1c = t.context.extractStorageLayout('StorageUpgrade_EndGap_V1c');
  const v2a = t.context.extractStorageLayout('StorageUpgrade_EndGap_V2a');
  const v2b = t.context.extractStorageLayout('StorageUpgrade_EndGap_V2b');

  t.deepEqual(getStorageUpgradeErrors(v1a, v2a), []);

  t.like(getStorageUpgradeErrors(v1b, v2a), {
    length: 1,
    0: {
      kind: 'delete',
      original: { label: '__gap' },
    },
  });

  t.like(getStorageUpgradeErrors(v1c, v2a), {
    length: 2,
    0: {
      kind: 'delete',
      original: { label: '__gap' },
    },
    1: {
      kind: 'delete',
      original: { label: 'b' },
    },
  });

  t.deepEqual(getStorageUpgradeErrors(v1b, v2b), []);
});

test('storage upgrade that consumes gap and inherited gap with smaller data type', t => {
  const v1a = t.context.extractStorageLayout('StorageUpgrade_EndGap_Uint128_V1a');
  const v1b = t.context.extractStorageLayout('StorageUpgrade_EndGap_Uint128_V1b');
  const v1c = t.context.extractStorageLayout('StorageUpgrade_EndGap_Uint128_V1c');
  const v2a = t.context.extractStorageLayout('StorageUpgrade_EndGap_Uint128_V2a');
  const v2b = t.context.extractStorageLayout('StorageUpgrade_EndGap_Uint128_V2b');

  t.deepEqual(getStorageUpgradeErrors(v1a, v2a), []);

  t.like(getStorageUpgradeErrors(v1b, v2a), {
    length: 1,
    0: {
      kind: 'delete',
      original: { label: '__gap' },
    },
  });

  t.like(getStorageUpgradeErrors(v1c, v2a), {
    length: 2,
    0: {
      kind: 'delete',
      original: { label: '__gap' },
    },
    1: {
      kind: 'delete',
      original: { label: 'b' },
    },
  });

  t.deepEqual(getStorageUpgradeErrors(v1b, v2b), []);
});

test('storage upgrade with different typed gaps', t => {
  const uint256gap_address_v1 = t.context.extractStorageLayout('StorageUpgrade_Uint256Gap_Address_V1');
  const uint256gap_address_v2 = t.context.extractStorageLayout('StorageUpgrade_Uint256Gap_Address_V2');

  const address_v1 = t.context.extractStorageLayout('StorageUpgrade_AddressGap_V1');
  const address_v2 = t.context.extractStorageLayout('StorageUpgrade_AddressGap_V2');

  const uint128_v1 = t.context.extractStorageLayout('StorageUpgrade_Uint128Gap_V1');
  const uint128_v2_ok = t.context.extractStorageLayout('StorageUpgrade_Uint128Gap_V2_Ok');
  const uint128_v2b_ok = t.context.extractStorageLayout('StorageUpgrade_Uint128Gap_V2b_Ok');
  const uint128_v2_bad = t.context.extractStorageLayout('StorageUpgrade_Uint128Gap_V2_Bad');

  const bool_v1 = t.context.extractStorageLayout('StorageUpgrade_BoolGap_V1');
  const bool_v2_ok = t.context.extractStorageLayout('StorageUpgrade_BoolGap_V2_Ok');
  const bool_v2_bad = t.context.extractStorageLayout('StorageUpgrade_BoolGap_V2_Bad');

  t.deepEqual(getStorageUpgradeErrors(uint256gap_address_v1, uint256gap_address_v2), []);
  t.deepEqual(getStorageUpgradeErrors(address_v1, address_v2), []);

  t.deepEqual(getStorageUpgradeErrors(uint128_v1, uint128_v2_ok), []);
  t.deepEqual(getStorageUpgradeErrors(uint128_v1, uint128_v2b_ok), []);

  t.like(getStorageUpgradeErrors(uint128_v1, uint128_v2_bad), {
    length: 2,
    0: {
      kind: 'layoutchange',
      change: {
        slot: {
          from: '1',
          to: '2',
        },
      },
      original: { label: '__gap' },
      updated: { label: '__gap' },
    },
    1: {
      kind: 'layoutchange',
      change: {
        slot: {
          from: '26',
          to: '27',
        },
      },
      original: { label: 'z' },
      updated: { label: 'z' },
    },
  });

  t.deepEqual(getStorageUpgradeErrors(bool_v1, bool_v2_ok), []);
  t.like(getStorageUpgradeErrors(bool_v1, bool_v2_bad), {
    length: 2,
    0: {
      kind: 'typechange',
      change: { kind: 'array shrink' },
      original: { label: '__gap' },
      updated: { label: '__gap' },
    },
    1: {
      kind: 'typechange',
      change: { kind: 'obvious mismatch' },
      original: { label: 'z' },
      updated: { label: 'z' },
    },
  });
});

test('storage upgrade with one element gap', t => {
  const v1 = t.context.extractStorageLayout('StorageUpgrade_Gap_One_Element_V1');
  const v2_Ok = t.context.extractStorageLayout('StorageUpgrade_Gap_One_Element_V2_Ok');
  const v2b_Ok = t.context.extractStorageLayout('StorageUpgrade_Gap_One_Element_V2b_Ok');
  const v2_Bad = t.context.extractStorageLayout('StorageUpgrade_Gap_One_Element_V2_Bad');

  t.deepEqual(getStorageUpgradeErrors(v1, v2_Ok), []);
  t.deepEqual(getStorageUpgradeErrors(v1, v2b_Ok), []);
  t.like(getStorageUpgradeErrors(v1, v2_Bad), {
    length: 2,
    0: {
      kind: 'layoutchange',
      change: {
        slot: {
          from: '2',
          to: '3',
        },
      },
      original: { label: '__gap' },
      updated: { label: '__gap' },
    },
    1: {
      kind: 'layoutchange',
      change: {
        slot: {
          from: '3',
          to: '4',
        },
      },
      original: { label: 'z' },
      updated: { label: 'z' },
    },
  });
});

test('storage upgrade with bool non-array named as __gap', t => {
  const v1 = t.context.extractStorageLayout('StorageUpgrade_Gap_Bool_Not_Array_V1');
  const v2_Bad = t.context.extractStorageLayout('StorageUpgrade_Gap_Bool_Not_Array_V2_Bad');

  t.like(getStorageUpgradeErrors(v1, v2_Bad), {
    length: 1,
    0: {
      kind: 'rename',
      original: { label: '__gap' },
      updated: { label: 'c' },
    },
  });
});

test('storage upgrade with uint256 non-array named as __gap', t => {
  const v1 = t.context.extractStorageLayout('StorageUpgrade_Gap_Uint256_Not_Array_V1');
  const v2_Bad = t.context.extractStorageLayout('StorageUpgrade_Gap_Uint256_Not_Array_V2_Bad');

  t.like(getStorageUpgradeErrors(v1, v2_Bad), {
    length: 1,
    0: {
      kind: 'rename',
      original: { label: '__gap' },
      updated: { label: 'c' },
    },
  });
});

test('storage upgrade with struct gap', t => {
  const v1 = t.context.extractStorageLayout('StorageUpgrade_StructGap_V1');
  const v2_Ok = t.context.extractStorageLayout('StorageUpgrade_StructGap_V2_Ok');
  const v2_Bad = t.context.extractStorageLayout('StorageUpgrade_StructGap_V2_Bad');

  t.deepEqual(getStorageUpgradeErrors(v1, v2_Ok), []);
  t.like(getStorageUpgradeErrors(v1, v2_Bad), {
    length: 3,
    0: {
      kind: 'typechange',
      original: { label: 'store_mapping' },
      updated: { label: 'store_mapping' },
    },
    1: {
      kind: 'typechange',
      original: { label: 'store_fixed_array' },
      updated: { label: 'store_fixed_array' },
    },
    2: {
      kind: 'typechange',
      original: { label: 'store_dynamic_array' },
      updated: { label: 'store_dynamic_array' },
    },
  });
});

test('storage upgrade with function pointers', t => {
  const v1 = t.context.extractStorageLayout('StorageUpgrade_FunctionPointer_V1');
  const v2_Ok = t.context.extractStorageLayout('StorageUpgrade_FunctionPointer_V2_Ok');
  const v2_Bad = t.context.extractStorageLayout('StorageUpgrade_FunctionPointer_V2_Bad');

  t.deepEqual(getStorageUpgradeErrors(v1, v2_Ok), []);

  t.like(getStorageUpgradeErrors(v1, v2_Bad), {
    length: 2,
    0: {
      kind: 'typechange',
      change: {
        kind: 'struct members',
        ops: {
          length: 1,
          0: {
            kind: 'typechange',
            change: {
              kind: 'visibility change',
            },
          },
        },
      },
      original: { label: 's' },
      updated: { label: 's' },
    },
    1: {
      kind: 'typechange',
      change: {
        kind: 'visibility change',
      },
      original: { label: 'c' },
      updated: { label: 'c' },
    },
  });
});
