import _test, { TestFn } from 'ava';
import assert from 'assert';

import { artifacts } from 'hardhat';

import { solcInputOutputDecoder } from './src-decoder';
import { validate, ValidationRunData } from './validate/run';
import { getStorageLayout } from './validate/query';
import { normalizeValidationData, ValidationData } from './validate/data';
import { StorageLayout } from './storage/layout';

import { Manifest, ManifestData } from './manifest';
import { getUpdatedStorageLayout, getStorageLayoutForAddress } from './manifest-storage-layout';

interface Context {
  validationRun: ValidationRunData;
  validationData: ValidationData;
}

const test = _test as TestFn<Context>;

test.before(async t => {
  const buildInfo = await artifacts.getBuildInfo('contracts/test/ManifestMigrate.sol:ManifestMigrateUnique');
  assert(buildInfo !== undefined, 'Build info not found');
  const decodeSrc = solcInputOutputDecoder(buildInfo.input, buildInfo.output);
  t.context.validationRun = validate(buildInfo.output, decodeSrc);
  t.context.validationData = normalizeValidationData([t.context.validationRun]);
});

test('getStorageLayoutForAddress - update layout', async t => {
  const { version } = t.context.validationRun['contracts/test/ManifestMigrate.sol:ManifestMigrateUnique'];
  assert(version !== undefined);
  const updatedLayout = getStorageLayout(t.context.validationData, version);
  const outdatedLayout = removeStorageLayoutMembers(updatedLayout);
  const address = '0xCf7Ed3AccA5a467e9e704C703E8D87F634fB0Fc9';
  const manifest = mockManifest({
    manifestVersion: '3.2',
    impls: {
      [version.withoutMetadata]: {
        address,
        txHash: '0x6580b51f3edcacacf30d7b4140e4022b65d2a5ba7cbe7e4d91397f4c3b5e8a6b',
        layout: outdatedLayout,
      },
    },
    proxies: [],
  });

  const layout = await getStorageLayoutForAddress(manifest, t.context.validationData, address);
  t.deepEqual(layout, updatedLayout);
  t.like(manifest.data, {
    impls: {
      [version.withoutMetadata]: {
        address,
        txHash: '0x6580b51f3edcacacf30d7b4140e4022b65d2a5ba7cbe7e4d91397f4c3b5e8a6b',
        layout: updatedLayout,
      },
    },
    proxies: [],
  });
});

test('getUpdatedLayout - unique layout match', async t => {
  const { version } = t.context.validationRun['contracts/test/ManifestMigrate.sol:ManifestMigrateUnique'];
  assert(version !== undefined);
  const targetLayout = getStorageLayout(t.context.validationData, version);
  const outdatedLayout = removeStorageLayoutMembers(targetLayout);
  const updatedLayout = getUpdatedStorageLayout(t.context.validationData, version.withoutMetadata, outdatedLayout);
  t.deepEqual(updatedLayout, targetLayout);
});

test('getUpdatedLayout - multiple unambiguous layout matches', async t => {
  const { version: version1 } =
    t.context.validationRun['contracts/test/ManifestMigrate.sol:ManifestMigrateUnambiguous1'];
  const { version: version2 } =
    t.context.validationRun['contracts/test/ManifestMigrate.sol:ManifestMigrateUnambiguous2'];
  assert(version1 !== undefined && version2 !== undefined);
  t.is(version1.withoutMetadata, version2.withoutMetadata, 'version is meant to be ambiguous');
  t.not(version1.withMetadata, version2.withMetadata, 'version with metadata should be different');
  const targetLayout = getStorageLayout(t.context.validationData, version1);
  const outdatedLayout = removeStorageLayoutMembers(targetLayout);
  const updatedLayout = getUpdatedStorageLayout(t.context.validationData, version1.withoutMetadata, outdatedLayout);
  t.deepEqual(updatedLayout, targetLayout);
});

test('getUpdatedLayout - multiple ambiguous layout matches', async t => {
  const { version: version1 } = t.context.validationRun['contracts/test/ManifestMigrate.sol:ManifestMigrateAmbiguous1'];
  const { version: version2 } = t.context.validationRun['contracts/test/ManifestMigrate.sol:ManifestMigrateAmbiguous2'];
  assert(version1 !== undefined && version2 !== undefined);
  t.is(version1.withoutMetadata, version2.withoutMetadata, 'version is meant to be ambiguous');
  t.not(version1.withMetadata, version2.withMetadata, 'version with metadata should be different');
  const targetLayout = getStorageLayout(t.context.validationData, version1);
  const outdatedLayout = removeStorageLayoutMembers(targetLayout);
  const updatedLayout = getUpdatedStorageLayout(t.context.validationData, version1.withoutMetadata, outdatedLayout);
  t.is(updatedLayout, undefined);
});

function mockManifest(data: ManifestData) {
  type Mocked = 'read' | 'write' | 'lockedRun';
  const manifest: Pick<Manifest, Mocked> & { data: ManifestData } = {
    data,
    async read() {
      return this.data;
    },
    async write(data: ManifestData) {
      this.data = data;
    },
    async lockedRun<T>(cb: () => Promise<T>) {
      return cb();
    },
  };
  return manifest as Manifest & { data: ManifestData };
}

// Simulate a layout from a version without struct/enum members
function removeStorageLayoutMembers(layout: StorageLayout): StorageLayout {
  const res = { ...layout, types: { ...layout.types } };
  for (const id in res.types) {
    res.types[id] = { ...layout.types[id], members: undefined };
  }
  return res;
}
