import test from 'ava';

import { promises as fs } from 'fs';
import { rimraf } from 'rimraf';
import path from 'path';
import os from 'os';
import sinon from 'sinon';

import { fetchOrDeploy, fetchOrDeployGetDeployment, mergeAddresses } from './impl-store';
import { getVersion } from './version';
import { stubProvider } from './stub-provider';
import { ImplDeployment } from './manifest';
import { RemoteDeploymentId } from './deployment';

test.before(async () => {
  process.chdir(await fs.mkdtemp(path.join(os.tmpdir(), 'upgrades-core-test-')));
});

test.after(async () => {
  await rimraf(process.cwd());
});

const version1 = getVersion('01');
const version2 = getVersion('02', '02');

test('deploys on cache miss', async t => {
  const provider = stubProvider();
  await fetchOrDeploy(version1, provider, provider.deploy);
  t.is(provider.deployCount, 1);
});

test('reuses on cache hit', async t => {
  const provider = stubProvider();
  const cachedDeploy = () => fetchOrDeploy(version1, provider, provider.deploy);
  const address1 = await cachedDeploy();
  const address2 = await cachedDeploy();
  t.is(provider.deployCount, 1);
  t.is(address2, address1);
});

test('does not reuse unrelated version', async t => {
  const provider = stubProvider();
  const address1 = await fetchOrDeploy(version1, provider, provider.deploy);
  const address2 = await fetchOrDeploy(version2, provider, provider.deploy);
  t.is(provider.deployCount, 2);
  t.not(address2, address1);
});

test('cleans up invalid deployment', async t => {
  const chainId = 1234;
  const provider1 = stubProvider(chainId);
  // create a deployment on a network
  await fetchOrDeploy(version1, provider1, provider1.deploy);
  // try to fetch it on a different network with same chainId
  const provider2 = stubProvider(chainId);
  await t.throwsAsync(fetchOrDeploy(version1, provider2, provider2.deploy));
  // the failed deployment has been cleaned up
  await fetchOrDeploy(version1, provider2, provider2.deploy);
});

test('merge addresses', async t => {
  const depl1 = { address: '0x1' } as ImplDeployment;
  const depl2 = { address: '0x2' } as ImplDeployment;

  const { address, allAddresses } = mergeAddresses(depl1, depl2);
  t.is(address, '0x1');
  t.true(unorderedEqual(allAddresses, ['0x1', '0x2']), allAddresses.toString());
});

test('merge multiple existing addresses', async t => {
  const depl1 = { address: '0x1', allAddresses: ['0x1a', '0x1b'] } as ImplDeployment;
  const depl2 = { address: '0x2' } as ImplDeployment;

  const { address, allAddresses } = mergeAddresses(depl1, depl2);
  t.is(address, '0x1');
  t.true(unorderedEqual(allAddresses, ['0x1', '0x1a', '0x1b', '0x2']), allAddresses.toString());
});

test('merge all addresses', async t => {
  const depl1 = { address: '0x1', allAddresses: ['0x1a', '0x1b'] } as ImplDeployment;
  const depl2 = { address: '0x2', allAddresses: ['0x2a', '0x2b'] } as ImplDeployment;

  const { address, allAddresses } = mergeAddresses(depl1, depl2);
  t.is(address, '0x1');
  t.true(unorderedEqual(allAddresses, ['0x1', '0x1a', '0x1b', '0x2', '0x2a', '0x2b']), allAddresses.toString());
});

function unorderedEqual(arr1: string[], arr2: string[]) {
  return arr1.every(i => arr2.includes(i)) && arr2.every(i => arr1.includes(i));
}

test('defender - replace tx hash for deployment', async t => {
  const provider = stubProvider();

  // create a pending deployment with id
  const fakeDeploy = await provider.deployPending();
  provider.removeContract(fakeDeploy.address);
  async function fakeDeployWithId() {
    return {
      ...fakeDeploy,
      txHash: '0x1',
      remoteDeploymentId: 'abc',
    };
  }

  const getDeploymentResponse1 = sinon.stub().returns({
    status: 'submitted',
    txHash: '0x1',
  });
  // let it timeout
  await t.throwsAsync(
    fetchOrDeployGetDeployment(
      version1,
      provider,
      fakeDeployWithId,
      { timeout: 1, pollingInterval: 0 },
      undefined,
      getDeploymentResponse1,
    ),
  );

  // make the contract code exist
  provider.addContract(fakeDeploy.address);

  // simulate a changed tx hash
  const getDeploymentResponse2 = sinon.stub().returns({
    status: 'completed',
    txHash: '0x2',
  });
  const deployment = await fetchOrDeployGetDeployment(
    version1,
    provider,
    fakeDeployWithId,
    { timeout: 1, pollingInterval: 0 },
    undefined,
    getDeploymentResponse2,
  );
  t.is(deployment.address, fakeDeploy.address);
  t.is(deployment.txHash, '0x2');
});

test('defender - address clash', async t => {
  const provider = stubProvider();

  // create a pending deployment with id
  const fakeDeploy = await provider.deployPending();
  provider.removeContract(fakeDeploy.address);

  async function deployment1() {
    return {
      ...fakeDeploy,
      txHash: '0x1',
      remoteDeploymentId: 'abc',
    };
  }

  const mockPendingDeployment = sinon.stub().returns({
    status: 'submitted',
    txHash: '0x1',
  });
  // let it timeout
  await t.throwsAsync(
    fetchOrDeployGetDeployment(
      version1,
      provider,
      deployment1,
      { timeout: 1, pollingInterval: 0 },
      undefined,
      mockPendingDeployment,
    ),
  );

  // simulate a new deployment with the same address, but different tx hash and deployment id
  async function deployment2() {
    return {
      ...fakeDeploy,
      txHash: '0x2',
      remoteDeploymentId: 'def',
    };
  }

  // deploy with a different version hash
  const error = await t.throwsAsync(
    fetchOrDeployGetDeployment(
      version2,
      provider,
      deployment2,
      { timeout: 1, pollingInterval: 0 },
      undefined,
      mockPendingDeployment,
    ),
  );

  t.true(
    error?.message.startsWith(`The deployment clashes with an existing one at ${fakeDeploy.address}`),
    error?.message,
  );
});

test('defender - merge avoids address clash, replaces deployment id', async t => {
  const provider = stubProvider();

  const MERGE = true;

  // create a pending deployment with id
  const fakeDeploy = await provider.deployPending();
  provider.removeContract(fakeDeploy.address);

  async function deployment1() {
    return {
      ...fakeDeploy,
      txHash: '0x1',
      remoteDeploymentId: 'abc',
    };
  }

  const getDeploymentResponse1 = sinon.stub().returns({
    status: 'submitted',
    txHash: '0x1',
  });
  // let it timeout
  await t.throwsAsync(
    fetchOrDeployGetDeployment(
      version1,
      provider,
      deployment1,
      { timeout: 1, pollingInterval: 0 },
      MERGE,
      getDeploymentResponse1,
    ),
  );

  // simulate a failed previous deployment
  const getDeploymentResponse2 = sinon.stub().returns({
    status: 'failed',
    txHash: '0x1',
  });

  // redeploy with a new deployment id
  async function deployment2() {
    return {
      ...fakeDeploy,
      txHash: '0x2',
      remoteDeploymentId: 'def',
    };
  }

  // make the contract code exist
  provider.addContract(fakeDeploy.address);

  const deployment = await fetchOrDeployGetDeployment(
    version1,
    provider,
    deployment2,
    { timeout: 1, pollingInterval: 0 },
    MERGE,
    getDeploymentResponse2,
  );

  t.is(deployment.address, fakeDeploy.address);
  t.is(deployment.txHash, '0x2');
  t.is((deployment as RemoteDeploymentId).remoteDeploymentId, 'def');
});
