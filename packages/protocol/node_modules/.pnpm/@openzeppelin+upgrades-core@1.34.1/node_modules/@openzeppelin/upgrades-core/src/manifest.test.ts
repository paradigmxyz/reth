import os from 'os';
import test, { ExecutionContext } from 'ava';
import { Manifest, ManifestData, normalizeManifestData } from './manifest';
import { promises as fs } from 'fs';
import path from 'path';

async function writeTestManifest(file: string, address?: string) {
  const testManifest = {
    manifestVersion: '3.2',
    impls: {},
    proxies: [
      {
        address: address ?? '0x123',
        txHash: '0x0',
        kind: 'uups',
      },
    ],
  };
  await fs.mkdir(path.dirname(file), { recursive: true });
  await fs.writeFile(file, JSON.stringify(testManifest, null, 2) + '\n');
}

async function deleteFile(t: ExecutionContext, file: string) {
  try {
    await fs.unlink(file);
  } catch (e: any) {
    if (!e.message.includes('ENOENT')) {
      t.fail(e);
    }
  }
}

async function assertOldName(t: ExecutionContext<unknown>, id: number) {
  await fs.access(`.openzeppelin/unknown-${id}.json`);
  t.throwsAsync(fs.access(`.openzeppelin/polygon-mumbai.json`));
}

async function assertNewName(t: ExecutionContext<unknown>, id: number) {
  t.throwsAsync(fs.access(`.openzeppelin/unknown-${id}.json`));
  await fs.access(`.openzeppelin/polygon-mumbai.json`);
}

async function deleteManifests(t: ExecutionContext<unknown>, id: number) {
  await deleteFile(t, `.openzeppelin/unknown-${id}.json`);
  await deleteFile(t, '.openzeppelin/polygon-mumbai.json');
}

test.serial('multiple manifests', async t => {
  const id = 80001;
  await deleteManifests(t, id);

  await writeTestManifest(`.openzeppelin/unknown-${id}.json`);
  await writeTestManifest(`.openzeppelin/polygon-mumbai.json`);

  const manifest = new Manifest(id);
  await manifest.lockedRun(async () => {
    await t.throwsAsync(() => manifest.read(), {
      message: new RegExp(
        `Network files with different names .openzeppelin/unknown-${id}.json and .openzeppelin/polygon-mumbai.json were found for the same network.`,
      ),
    });
  });

  await deleteManifests(t, id);
});

test.serial('rename manifest', async t => {
  const id = 80001;
  await deleteManifests(t, id);

  await writeTestManifest(`.openzeppelin/unknown-${id}.json`);

  const manifest = new Manifest(id);
  t.is(manifest.file, `.openzeppelin/polygon-mumbai.json`);

  t.throwsAsync(fs.access(`.openzeppelin/chain-${id}.lock`));

  await assertOldName(t, id);
  await manifest.lockedRun(async () => {
    await fs.access(`.openzeppelin/chain-${id}.lock`);
    const data = await manifest.read();
    data.proxies.push({
      address: '0x456',
      txHash: '0x0',
      kind: 'uups',
    });

    await assertOldName(t, id);
    await manifest.write(data);
    await assertNewName(t, id);
  });
  await assertNewName(t, id);

  t.throwsAsync(fs.access(`.openzeppelin/chain-${id}.lock`));

  // check that the contents were persisted
  const data = await new Manifest(id).read();
  t.is(data.proxies[0].address, '0x123');
  t.is(data.proxies[1].address, '0x456');
  await assertNewName(t, id);

  t.throwsAsync(fs.access(`.openzeppelin/chain-${id}.lock`));

  await deleteManifests(t, id);
});

// This test is not working on some Linux configurations ('EXDEV: cross-device link not permitted')
test.skip('rename hardhat from unknown to dev manifest', async t => {
  const id = 31337;

  await deleteFile(t, `.openzeppelin/unknown-${id}.json`);
  await writeTestManifest(`.openzeppelin/unknown-${id}.json`);

  const instanceId = '0xaa0';
  const devInstanceMetadata = { networkName: 'hardhat', instanceId: instanceId };

  const manifest = new Manifest(id, devInstanceMetadata, os.tmpdir());

  await fs.access(`.openzeppelin/unknown-${id}.json`);
  await manifest.lockedRun(async () => {
    await fs.access(`${os.tmpdir()}/openzeppelin-upgrades/chain-${id}-${instanceId}.lock`);
    const data = await manifest.read();
    data.proxies.push({
      address: '0x456',
      txHash: '0x0',
      kind: 'uups',
    });

    await fs.access(`.openzeppelin/unknown-${id}.json`);
    await manifest.write(data);
  });
  t.throwsAsync(fs.access(`.openzeppelin/unknown-${id}.json`));
  await fs.access(`${os.tmpdir()}/openzeppelin-upgrades/hardhat-${id}-${instanceId}.json`);

  const dev = await new Manifest(id, devInstanceMetadata, os.tmpdir()).read();
  t.is(dev.proxies.length, 2);
  t.is(dev.proxies[0].address, '0x123');
  t.is(dev.proxies[1].address, '0x456');

  await deleteFile(t, `.openzeppelin/unknown-${id}.json`);
  await deleteFile(t, `${os.tmpdir()}/openzeppelin-upgrades/hardhat-${id}-${instanceId}.json`);
});

test.serial('forked chain from known network with fallback name', async t => {
  const forkedId = 80001;
  const devId = 55555;

  await deleteManifests(t, forkedId);

  await writeTestManifest(`.openzeppelin/unknown-${forkedId}.json`);

  const instanceId = '0xaaa';
  const devInstanceMetadata = { networkName: 'hardhat', instanceId: instanceId, forkedNetwork: { chainId: forkedId } };

  const manifest = new Manifest(devId, devInstanceMetadata, os.tmpdir());

  await assertOldName(t, forkedId);
  await manifest.lockedRun(async () => {
    await fs.access(`${os.tmpdir()}/openzeppelin-upgrades/chain-${devId}-${instanceId}.lock`);
    const data = await manifest.read();
    data.proxies.push({
      address: '0x456',
      txHash: '0x0',
      kind: 'uups',
    });

    await assertOldName(t, forkedId);
    await manifest.write(data);
    await assertOldName(t, forkedId); // original network file should not be changed
  });
  t.throwsAsync(fs.access(`.openzeppelin/chain-${forkedId}.lock`));
  t.throwsAsync(fs.access(`${os.tmpdir()}/openzeppelin-upgrades/chain-${devId}-${instanceId}.lock`));
  await fs.access(`${os.tmpdir()}/openzeppelin-upgrades/hardhat-${devId}-${instanceId}.json`);

  // check that the contents were NOT persisted to original manifest
  const orig = await new Manifest(forkedId).read();
  t.is(orig.proxies.length, 1);
  t.is(orig.proxies[0].address, '0x123');

  // check that the contents were persisted to dev copy of manifest
  const dev = await new Manifest(devId, devInstanceMetadata, os.tmpdir()).read();
  t.is(dev.proxies.length, 2);
  t.is(dev.proxies[0].address, '0x123');
  t.is(dev.proxies[1].address, '0x456');

  await deleteManifests(t, forkedId);
  await deleteFile(t, `${os.tmpdir()}/openzeppelin-upgrades/hardhat-${devId}-${instanceId}.json`);
});

test.serial('forked chain from known network with actual name', async t => {
  const forkedId = 80001;
  const devId = 55555;

  await deleteManifests(t, forkedId);

  await writeTestManifest(`.openzeppelin/polygon-mumbai.json`);

  const instanceId = '0xbbb';
  const devInstanceMetadata = { networkName: 'hardhat', instanceId: instanceId, forkedNetwork: { chainId: forkedId } };

  const manifest = new Manifest(devId, devInstanceMetadata, os.tmpdir());

  await assertNewName(t, forkedId);
  await manifest.lockedRun(async () => {
    await fs.access(`${os.tmpdir()}/openzeppelin-upgrades/chain-${devId}-${instanceId}.lock`);
    const data = await manifest.read();
    data.proxies.push({
      address: '0x456',
      txHash: '0x0',
      kind: 'uups',
    });

    await assertNewName(t, forkedId);
    await manifest.write(data);
    await assertNewName(t, forkedId); // original network file should not be changed
  });
  t.throwsAsync(fs.access(`.openzeppelin/chain-${forkedId}.lock`));
  t.throwsAsync(fs.access(`${os.tmpdir()}/openzeppelin-upgrades/chain-${devId}-${instanceId}.lock`));
  await fs.access(`${os.tmpdir()}/openzeppelin-upgrades/hardhat-${devId}-${instanceId}.json`);

  // check that the contents were NOT persisted to original manifest
  const orig = await new Manifest(forkedId).read();
  t.is(orig.proxies.length, 1);
  t.is(orig.proxies[0].address, '0x123');

  // check that the contents were persisted to dev copy of manifest
  const dev = await new Manifest(devId, devInstanceMetadata, os.tmpdir()).read();
  t.is(dev.proxies.length, 2);
  t.is(dev.proxies[0].address, '0x123');
  t.is(dev.proxies[1].address, '0x456');

  await deleteManifests(t, forkedId);
  await deleteFile(t, `${os.tmpdir()}/openzeppelin-upgrades/hardhat-${devId}-${instanceId}.json`);
});

test.serial('forked chain, real manifest already locked', async t => {
  const forkedId = 80001;
  const devId = 55555;

  const instanceId = '0xccc';
  const devInstanceMetadata = { networkName: 'hardhat', instanceId: instanceId, forkedNetwork: { chainId: forkedId } };

  const realManifest = new Manifest(forkedId);
  const devManifest = new Manifest(devId, devInstanceMetadata, os.tmpdir());

  try {
    await realManifest.lockedRun(async () => {
      await devManifest.read(0);
    });
    t.fail();
  } catch (e: any) {
    t.is(e.code, 'ELOCKED');
  }
});

test.serial('forked chain, real manifest already locked but dev manifest exists', async t => {
  const forkedId = 80001;
  const devId = 55555;

  const instanceId = '0xddd';
  const devInstanceMetadata = { networkName: 'hardhat', instanceId: instanceId, forkedNetwork: { chainId: forkedId } };

  const realManifest = new Manifest(forkedId);
  const devManifest = new Manifest(devId, devInstanceMetadata, os.tmpdir());

  // write dev manifest
  await writeTestManifest(devManifest.file, '0x999');

  // then write real manifest
  await deleteManifests(t, forkedId);
  await writeTestManifest(`.openzeppelin/polygon-mumbai.json`);

  let devManifestContents;
  await realManifest.lockedRun(async () => {
    // should ignore real manifest and real lock file
    devManifestContents = await devManifest.read();
    t.is(devManifestContents.proxies.length, 1);
    t.is(devManifestContents.proxies[0].address, '0x999');
  });

  await deleteManifests(t, forkedId);
  await deleteFile(t, `${os.tmpdir()}/openzeppelin-upgrades/hardhat-${devId}-${instanceId}.json`);
});

test.serial('forked chain without existing manifest', async t => {
  const forkedId = 80001;
  const devId = 55555;

  await deleteManifests(t, forkedId);

  const instanceId = '0xeee';
  const devInstanceMetadata = { networkName: 'hardhat', instanceId: instanceId, forkedNetwork: { chainId: forkedId } };

  const manifest = new Manifest(devId, devInstanceMetadata, os.tmpdir());

  await manifest.lockedRun(async () => {
    await fs.access(`${os.tmpdir()}/openzeppelin-upgrades/chain-${devId}-${instanceId}.lock`);
    const data = await manifest.read();
    data.proxies.push({
      address: '0x456',
      txHash: '0x0',
      kind: 'uups',
    });

    await manifest.write(data);
  });
  t.throwsAsync(fs.access(`${os.tmpdir()}/openzeppelin-upgrades/chain-${devId}-${instanceId}.lock`));
  await fs.access(`${os.tmpdir()}/openzeppelin-upgrades/hardhat-${devId}-${instanceId}.json`);

  // check that the contents were NOT persisted to regular manifest
  t.throwsAsync(fs.access(`.openzeppelin/unknown-${forkedId}.json`));
  t.throwsAsync(fs.access(`.openzeppelin/polygon-mumbai.json`));

  // check that the contents were persisted to dev copy of manifest
  const dev = await new Manifest(devId, devInstanceMetadata, os.tmpdir()).read();
  t.is(dev.proxies.length, 1);
  t.is(dev.proxies[0].address, '0x456');

  await deleteManifests(t, forkedId);
  await deleteFile(t, `${os.tmpdir()}/openzeppelin-upgrades/hardhat-${devId}-${instanceId}.json`);
});

test.serial('dev instance with known network id', async t => {
  const devId = 80001;

  await deleteManifests(t, devId);

  // dev instance without forking, so this real manifest should not be actually used
  await writeTestManifest(`.openzeppelin/polygon-mumbai.json`);

  const instanceId = '0xfff';
  const devInstanceMetadata = { networkName: 'hardhat', instanceId: instanceId };

  const manifest = new Manifest(devId, devInstanceMetadata, os.tmpdir());

  await manifest.lockedRun(async () => {
    await fs.access(`${os.tmpdir()}/openzeppelin-upgrades/chain-${devId}-${instanceId}.lock`);
    const data = await manifest.read();
    data.proxies.push({
      address: '0x456',
      txHash: '0x0',
      kind: 'uups',
    });

    await manifest.write(data);
  });
  t.throwsAsync(fs.access(`${os.tmpdir()}/openzeppelin-upgrades/chain-${devId}-${instanceId}.lock`));
  await fs.access(`${os.tmpdir()}/openzeppelin-upgrades/hardhat-${devId}-${instanceId}.json`);

  // check that the contents were NOT persisted to original manifest
  const orig = await new Manifest(devId).read();
  t.is(orig.proxies.length, 1);
  t.is(orig.proxies[0].address, '0x123');

  // check that the contents were persisted to dev manifest without original contents
  const dev = await new Manifest(devId, devInstanceMetadata, os.tmpdir()).read();
  t.is(dev.proxies.length, 1);
  t.is(dev.proxies[0].address, '0x456');

  await deleteManifests(t, devId);
  await deleteFile(t, `${os.tmpdir()}/openzeppelin-upgrades/hardhat-${devId}-${instanceId}.json`);
});

test('manifest name for a known network', t => {
  const manifest = new Manifest(1);
  t.is(manifest.file, '.openzeppelin/mainnet.json');
});

test('manifest name for a known network, development instance', t => {
  const chainId = 1;
  const instanceId = '0x11111';
  const devInstanceMetadata = { networkName: 'hardhat', instanceId: instanceId };

  const manifest = new Manifest(chainId, devInstanceMetadata, '/tmp');

  const expectedPath = `/tmp/openzeppelin-upgrades/hardhat-${chainId}-${instanceId}.json`;
  t.is(manifest.file, expectedPath);
  t.is(manifest.fallbackFile, expectedPath);
});

test('manifest name for an unknown network', t => {
  const id = 55555;
  const manifest = new Manifest(id);
  t.is(manifest.file, `.openzeppelin/unknown-${id}.json`);
});

test('manifest name for an unknown network, development instance, non hardhat', t => {
  const chainId = 55555;
  const instanceId = '0x22222';
  const devInstanceMetadata = { networkName: 'dev', instanceId: instanceId };

  const manifest = new Manifest(chainId, devInstanceMetadata, '/tmp');

  const expectedPath = `/tmp/openzeppelin-upgrades/dev-${chainId}-${instanceId}.json`;
  t.is(manifest.file, expectedPath);
  t.is(manifest.fallbackFile, expectedPath);
});

test('manifest name for an unknown network, development instance, hardhat', t => {
  const chainId = 31337;
  const instanceId = '0x22223';
  const devInstanceMetadata = { networkName: 'hardhat', instanceId: instanceId };

  const manifest = new Manifest(chainId, devInstanceMetadata, '/tmp');

  const expectedPath = `/tmp/openzeppelin-upgrades/hardhat-${chainId}-${instanceId}.json`;
  t.is(manifest.file, expectedPath);
  t.is(manifest.fallbackFile, `.openzeppelin/unknown-${chainId}.json`);
});

test('manifest name for an unknown network, development instance, anvil - forkedNetwork null', t => {
  const chainId = 31337;
  const instanceId = '0x22224';
  const devInstanceMetadata = { networkName: 'anvil', instanceId: instanceId, forkedNetwork: null };

  const manifest = new Manifest(chainId, devInstanceMetadata, '/tmp');

  const expectedPath = `/tmp/openzeppelin-upgrades/anvil-${chainId}-${instanceId}.json`;
  t.is(manifest.file, expectedPath);
  t.is(manifest.fallbackFile, `.openzeppelin/unknown-${chainId}.json`);
});

test('manifest dev instance without tmp dir param', t => {
  const chainId = 1;
  const instanceId = '0x33333';
  const devInstanceMetadata = { networkName: 'hardhat', instanceId: instanceId };

  t.throws(() => new Manifest(chainId, devInstanceMetadata));
});

test('manifest lock file name for a development network instance', async t => {
  const chainId = 55555;
  const instanceId = '0x55555';
  const devInstanceMetadata = { networkName: 'hardhat', instanceId: instanceId };

  const manifest = new Manifest(chainId, devInstanceMetadata, '/tmp');

  await manifest.lockedRun(async () => {
    t.throwsAsync(fs.access(`.openzeppelin/chain-${chainId}.lock`));
    t.throwsAsync(fs.access(`.openzeppelin/chain-${chainId}-${instanceId}.lock`));
    t.throwsAsync(fs.access(`/tmp/openzeppelin-upgrades/chain-${chainId}.lock`));
    await fs.access(`/tmp/openzeppelin-upgrades/chain-${chainId}-${instanceId}.lock`);
  });
});

test('normalize manifest', t => {
  const deployment = {
    address: '0x1234',
    txHash: '0x1234',
    remoteDeploymentId: 'abc',
    kind: 'uups' as const,
    layout: { solcVersion: '0.8.9', types: {}, storage: [] },
    deployTransaction: {},
  };
  const input: ManifestData = {
    manifestVersion: '3.0',
    admin: deployment,
    impls: { a: deployment },
    proxies: [deployment],
  };
  const norm = normalizeManifestData(input);
  t.like(norm.admin, {
    ...deployment,
    kind: undefined,
    layout: undefined,
    deployTransaction: undefined,
  });
  t.like(norm.impls.a, {
    ...deployment,
    kind: undefined,
    deployTransaction: undefined,
  });
  t.like(norm.proxies[0], {
    ...deployment,
    layout: undefined,
    deployTransaction: undefined,
  });
});
