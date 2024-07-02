import _test, { TestFn } from 'ava';
import { findAll, astDereferencer } from 'solidity-ast/utils';
import { artifacts } from 'hardhat';

import { SolcOutput } from './solc-api';
import { getStorageUpgradeErrors } from './storage';
import { extractStorageLayout } from './storage/extract';
import { BuildInfo } from 'hardhat/types';
import { stabilizeStorageLayout } from './utils/stabilize-layout';

interface Context {
  extractStorageLayout: (contract: string, withLayout?: boolean) => Promise<ReturnType<typeof extractStorageLayout>>;
}

const test = _test as TestFn<Context>;

test.before(async t => {
  const buildInfoCache: Record<string, BuildInfo | undefined> = {};
  t.context.extractStorageLayout = async (contract, withLayout = true) => {
    const [file] = contract.split('_');
    const source = `contracts/test/${file}.sol`;
    const buildInfo = (buildInfoCache[source] ??= await artifacts.getBuildInfo(`${source}:${contract}`));
    if (buildInfo === undefined) {
      throw new Error(`Build info for ${source} not found`);
    }
    const solcOutput: SolcOutput = buildInfo.output;
    for (const def of findAll('ContractDefinition', solcOutput.sources[source].ast)) {
      // eslint-disable-next-line @typescript-eslint/no-non-null-assertion
      const layout = solcOutput.contracts[source][def.name].storageLayout!;
      if (def.name === contract) {
        return extractStorageLayout(def, dummyDecodeSrc, astDereferencer(solcOutput), withLayout ? layout : undefined);
      }
    }
    throw new Error(`Contract ${contract} not found in ${source}`);
  };
});

const dummyDecodeSrc = () => 'file.sol:1';

test('user defined value types - extraction - 0.8.8', async t => {
  const layout = await t.context.extractStorageLayout('Storage088');
  t.snapshot(stabilizeStorageLayout(layout));
});

test('user defined value types - extraction - 0.8.9', async t => {
  const layout = await t.context.extractStorageLayout('Storage089');
  t.snapshot(stabilizeStorageLayout(layout));
});

test('user defined value types - bad upgrade from 0.8.8 to 0.8.9', async t => {
  const v1 = await t.context.extractStorageLayout('Storage088');
  const v2 = await t.context.extractStorageLayout('Storage089');
  const comparison = getStorageUpgradeErrors(v1, v2);
  t.like(comparison, {
    length: 1,
    0: {
      kind: 'typechange',
      change: {
        kind: 'type resize',
      },
      original: { label: 'my_user_value' },
      updated: { label: 'my_user_value' },
    },
  });
});

test('user defined value types - valid upgrade', async t => {
  const v1 = await t.context.extractStorageLayout('Storage089');
  const v2 = await t.context.extractStorageLayout('Storage089_V2');
  const comparison = getStorageUpgradeErrors(v1, v2);
  t.deepEqual(comparison, []);
});

test('user defined value types - no layout info', async t => {
  const layout = await t.context.extractStorageLayout('Storage089', false);
  t.snapshot(stabilizeStorageLayout(layout));
});

test('user defined value types - no layout info - from 0.8.8 to 0.8.9', async t => {
  const v1 = await t.context.extractStorageLayout('Storage088', false);
  const v2 = await t.context.extractStorageLayout('Storage089', false);
  const comparison = getStorageUpgradeErrors(v1, v2);
  t.deepEqual(comparison, []);
});

test('user defined value types comparison - no layout info', async t => {
  const v1 = await t.context.extractStorageLayout('Storage089', false);
  const v2 = await t.context.extractStorageLayout('Storage089_V2', false);
  const comparison = getStorageUpgradeErrors(v1, v2);
  t.deepEqual(comparison, []);
});

test('user defined value types - no layout info - bad underlying type', async t => {
  const v1 = await t.context.extractStorageLayout('Storage089', false);
  const v2 = await t.context.extractStorageLayout('Storage089_V3', false);
  const comparison = getStorageUpgradeErrors(v1, v2);
  t.like(comparison, {
    length: 1,
    0: {
      kind: 'typechange',
      change: {
        kind: 'unknown',
      },
      original: { label: 'my_user_value' },
      updated: { label: 'my_user_value' },
    },
  });
});

test('renamed retyped - extraction', async t => {
  const layout = await t.context.extractStorageLayout('StorageRenamedRetyped');
  t.snapshot(stabilizeStorageLayout(layout));
});

test('mapping with user defined value type key - ok', async t => {
  const v1 = await t.context.extractStorageLayout('Storage089_MappingUVDTKey_V1');
  const v2 = await t.context.extractStorageLayout('Storage089_MappingUVDTKey_V2_Ok');
  const comparison = getStorageUpgradeErrors(v1, v2);
  t.deepEqual(comparison, []);
});

test('mapping with user defined value type key - bad', async t => {
  const v1 = await t.context.extractStorageLayout('Storage089_MappingUVDTKey_V1');
  const v2 = await t.context.extractStorageLayout('Storage089_MappingUVDTKey_V2_Bad');
  const comparison = getStorageUpgradeErrors(v1, v2);
  t.like(comparison, {
    length: 2,
    0: {
      kind: 'typechange',
      change: {
        kind: 'mapping key',
      },
      original: { label: 'm1' },
      updated: { label: 'm1' },
    },
    1: {
      kind: 'typechange',
      change: {
        kind: 'mapping key',
      },
      original: { label: 'm2' },
      updated: { label: 'm2' },
    },
  });
});
