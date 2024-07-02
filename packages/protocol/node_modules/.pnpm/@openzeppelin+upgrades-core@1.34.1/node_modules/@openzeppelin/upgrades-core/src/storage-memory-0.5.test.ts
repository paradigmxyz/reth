import _test, { TestFn } from 'ava';
import { findAll, astDereferencer } from 'solidity-ast/utils';
import { artifacts } from 'hardhat';

import { SolcOutput } from './solc-api';
import { getStorageUpgradeErrors } from './storage';
import { extractStorageLayout } from './storage/extract';
import { BuildInfo } from 'hardhat/types';
import { stabilizeStorageLayout } from './utils/stabilize-layout';

interface Context {
  extractStorageLayout: (
    contract: string,
    contractName?: string,
    withLayout?: boolean,
  ) => Promise<ReturnType<typeof extractStorageLayout>>;
}

const test = _test as TestFn<Context>;

test.before(async t => {
  const buildInfoCache: Record<string, BuildInfo | undefined> = {};
  t.context.extractStorageLayout = async (contractFile, contractName, withLayout = true) => {
    const expectedContract = contractName ?? contractFile;

    const [file] = contractFile.split('_');
    const source = `contracts/test/${file}.sol`;
    const buildInfo = (buildInfoCache[source] ??= await artifacts.getBuildInfo(`${source}:${expectedContract}`));
    if (buildInfo === undefined) {
      throw new Error(`Build info for ${source} not found`);
    }
    const solcOutput: SolcOutput = buildInfo.output;
    for (const def of findAll('ContractDefinition', solcOutput.sources[source].ast)) {
      // eslint-disable-next-line @typescript-eslint/no-non-null-assertion
      const layout = solcOutput.contracts[source][def.name].storageLayout!;
      if (def.name === expectedContract) {
        return extractStorageLayout(def, dummyDecodeSrc, astDereferencer(solcOutput), withLayout ? layout : undefined);
      }
    }
    throw new Error(`Contract ${expectedContract} not found in ${source}`);
  };
});

const dummyDecodeSrc = () => 'file.sol:1';

test('memory 0.5.16', async t => {
  const layout = await t.context.extractStorageLayout('Memory05');
  t.snapshot(stabilizeStorageLayout(layout));
});

test('memory 0.8.9', async t => {
  const layout = await t.context.extractStorageLayout('Memory08');
  t.snapshot(stabilizeStorageLayout(layout));
});

test('memory - upgrade from 0.5.16 to 0.8.9', async t => {
  const v1 = await t.context.extractStorageLayout('Memory05');
  const v2 = await t.context.extractStorageLayout('Memory08');

  const comparison = getStorageUpgradeErrors(v1, v2);
  t.deepEqual(comparison, []);
});

test('memory - bad upgrade from 0.5.16 to 0.8.9', async t => {
  const v1 = await t.context.extractStorageLayout('Memory05');
  const v2 = await t.context.extractStorageLayout('Memory08', 'Memory08Bad');

  const comparison = getStorageUpgradeErrors(v1, v2);
  t.like(comparison, {
    length: 2,
    0: {
      kind: 'typechange',
      change: {
        kind: 'mapping key',
      },
      original: { label: 'a' },
      updated: { label: 'a' },
    },
    1: {
      kind: 'typechange',
      change: {
        kind: 'mapping key',
      },
      original: { label: 'b' },
      updated: { label: 'b' },
    },
  });
});
