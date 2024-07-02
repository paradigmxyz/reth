import test from 'ava';
import { promisify } from 'util';
import { TransactionMinedTimeout } from '.';
import sinon from 'sinon';

import { Deployment, RemoteDeploymentId, resumeOrDeploy, waitAndValidateDeployment } from './deployment';
import { stubProvider } from './stub-provider';

const sleep = promisify(setTimeout);

test.afterEach.always(() => {
  sinon.restore();
});

test('deploys new contract', async t => {
  const provider = stubProvider();
  const deployment = await resumeOrDeploy(provider, undefined, provider.deploy);
  t.true(provider.isContract(deployment.address));
  t.is(provider.deployCount, 1);
});

test('resumes existing deployment with txHash', async t => {
  const provider = stubProvider();
  const first = await resumeOrDeploy(provider, undefined, provider.deployPending);
  const second = await resumeOrDeploy(provider, first, provider.deployPending);
  t.is(first, second);
  t.is(provider.deployCount, 1);
});

test('resumes existing deployment without txHash', async t => {
  const provider = stubProvider();
  const first = await provider.deploy();
  delete first.txHash;
  const second = await resumeOrDeploy(provider, first, provider.deployPending);
  t.is(first, second);
  t.is(provider.deployCount, 1);
});

test('errors if tx is not found', async t => {
  const provider = stubProvider();
  const fakeDeployment: Deployment = {
    address: '0x1aec6468218510f19bb19f52c4767996895ce711',
    txHash: '0xc48e21ac9c051922f5ccf1b47b62000f567ef9bbc108d274848b44351a6872cb',
  };
  await t.throwsAsync(resumeOrDeploy(provider, fakeDeployment, provider.deploy));
});

test('redeploys if tx is not found on dev network', async t => {
  // 31337 = Hardhat Network chainId
  const provider = stubProvider(31337, 'HardhatNetwork/2.2.1/@ethereumjs/vm/5.3.2');
  const fakeDeployment: Deployment = {
    address: '0x1aec6468218510f19bb19f52c4767996895ce711',
    txHash: '0xc48e21ac9c051922f5ccf1b47b62000f567ef9bbc108d274848b44351a6872cb',
  };
  const deployment = await resumeOrDeploy(provider, fakeDeployment, provider.deploy);
  t.true(provider.isContract(deployment.address));
  t.is(provider.deployCount, 1);
});

test('validates a mined deployment with txHash', async t => {
  const provider = stubProvider();
  const deployment = await resumeOrDeploy(provider, undefined, provider.deploy);
  await waitAndValidateDeployment(provider, deployment);
  t.is(provider.getMethodCount('eth_getTransactionReceipt'), 1);
  t.is(provider.getMethodCount('eth_getCode'), 1);
});

test('validates a mined deployment without txHash', async t => {
  const provider = stubProvider();
  const deployment = await resumeOrDeploy(provider, undefined, provider.deploy);
  delete deployment.txHash;
  await waitAndValidateDeployment(provider, deployment);
  t.is(provider.getMethodCount('eth_getTransactionReceipt'), 0);
  t.is(provider.getMethodCount('eth_getCode'), 1);
});

test('waits for a deployment to mine', async t => {
  const timeout = Symbol('timeout');
  const provider = stubProvider();
  const deployment = await resumeOrDeploy(provider, undefined, provider.deployPending);
  const result = await Promise.race([waitAndValidateDeployment(provider, deployment), sleep(100).then(() => timeout)]);
  t.is(result, timeout);
  provider.mine();
  await waitAndValidateDeployment(provider, deployment);
});

test('fails deployment fast if tx reverts', async t => {
  const provider = stubProvider();
  const deployment = await resumeOrDeploy(provider, undefined, provider.deploy);
  // eslint-disable-next-line @typescript-eslint/no-non-null-assertion
  provider.failTx(deployment.txHash!);
  await t.throwsAsync(waitAndValidateDeployment(provider, deployment));
});

test('waits for a deployment to return contract code', async t => {
  const timeout = Symbol('timeout');
  const provider = stubProvider();
  const deployment = await resumeOrDeploy(provider, undefined, provider.deploy);
  provider.removeContract(deployment.address);
  const result = await Promise.race([waitAndValidateDeployment(provider, deployment), sleep(100).then(() => timeout)]);
  t.is(result, timeout);
  provider.addContract(deployment.address);
  await waitAndValidateDeployment(provider, deployment);
});

test('tx mined timeout - no params', async t => {
  const provider = stubProvider();
  const deployment = await resumeOrDeploy(provider, undefined, provider.deploy);
  try {
    throw new TransactionMinedTimeout(deployment);
  } catch (e: any) {
    const EXPECTED =
      /Timed out waiting for contract deployment to address \S+ with transaction \S+\n\nRun the function again to continue waiting for the transaction confirmation./;
    t.true(EXPECTED.test(e.message), e.message);
  }
});

test('tx mined timeout - params', async t => {
  const provider = stubProvider();
  const deployment = await resumeOrDeploy(provider, undefined, provider.deploy);
  try {
    throw new TransactionMinedTimeout(deployment, 'implementation', true);
  } catch (e: any) {
    const EXPECTED =
      /Timed out waiting for implementation contract deployment to address \S+ with transaction \S+\n\nRun the function again to continue waiting for the transaction confirmation. If the problem persists, adjust the polling parameters with the timeout and pollingInterval options./;
    t.true(EXPECTED.test(e.message), e.message);
  }
});

test('platform - resumes existing deployment id and replaces tx hash', async t => {
  const provider = stubProvider();

  const getDeploymentResponse = sinon.stub().returns({
    status: 'submitted',
    txHash: '0x2',
  });

  const first: Deployment & RemoteDeploymentId = await resumeOrDeploy(provider, undefined, provider.deployPending);
  first.txHash = '0x1';
  first.remoteDeploymentId = 'abc';

  const second: Deployment & RemoteDeploymentId = await resumeOrDeploy(
    provider,
    first,
    provider.deployPending,
    undefined,
    undefined,
    undefined,
    undefined,
    getDeploymentResponse,
  );
  t.is(second.address, first.address);
  t.is(second.remoteDeploymentId, first.remoteDeploymentId);
  t.is(second.txHash, '0x2');
  t.is(provider.deployCount, 1);
});

test('platform - resumes existing deployment id and uses tx hash', async t => {
  const provider = stubProvider();

  const getDeploymentResponse = sinon.stub().throws();

  const first: Deployment & RemoteDeploymentId = await resumeOrDeploy(provider, undefined, provider.deploy);
  // tx hash was mined
  first.remoteDeploymentId = 'abc';

  const second: Deployment & RemoteDeploymentId = await resumeOrDeploy(
    provider,
    first,
    provider.deployPending,
    undefined,
    undefined,
    undefined,
    undefined,
    getDeploymentResponse,
  );
  t.is(second.address, first.address);
  t.is(second.txHash, first.txHash);
  t.is(second.remoteDeploymentId, first.remoteDeploymentId);
  t.is(provider.deployCount, 1);

  await waitAndValidateDeployment(provider, second);
  t.is(provider.getMethodCount('eth_getTransactionReceipt'), 1);
});

test('platform - errors if tx and deployment id are not found', async t => {
  const provider = stubProvider();

  const getDeploymentResponse = sinon.stub().returns(undefined);

  const fakeDeployment: Deployment & RemoteDeploymentId = {
    address: '0x1aec6468218510f19bb19f52c4767996895ce711',
    txHash: '0xc48e21ac9c051922f5ccf1b47b62000f567ef9bbc108d274848b44351a6872cb',
    remoteDeploymentId: 'abc',
  };

  await t.throwsAsync(
    resumeOrDeploy(
      provider,
      fakeDeployment,
      provider.deploy,
      undefined,
      undefined,
      undefined,
      undefined,
      getDeploymentResponse,
    ),
  );
  t.true(getDeploymentResponse.calledOnceWithExactly('abc'));
});

test('platform - waits for a deployment to be completed', async t => {
  const timeout = Symbol('timeout');
  const provider = stubProvider();

  const deployment: Deployment & RemoteDeploymentId = await provider.deployPending();
  deployment.remoteDeploymentId = 'abc';
  provider.removeContract(deployment.address);

  const getDeploymentResponse = sinon.stub();
  getDeploymentResponse.onCall(0).returns({
    status: 'submitted',
    txHash: deployment.txHash,
  });
  getDeploymentResponse.onCall(1).returns({
    status: 'completed',
    txHash: deployment.txHash,
  });

  // checks that status is completed but code not at address yet
  const result = await Promise.race([
    waitAndValidateDeployment(provider, deployment, undefined, { pollingInterval: 0 }, getDeploymentResponse),
    sleep(100).then(() => timeout),
  ]);
  t.is(result, timeout);
  t.is(getDeploymentResponse.callCount, 2);

  // code mined to address
  provider.addContract(deployment.address);
  provider.mine();

  // checks that the code is at address
  await waitAndValidateDeployment(provider, deployment, undefined, { pollingInterval: 0 }, getDeploymentResponse);

  t.is(getDeploymentResponse.callCount, 2);
});

test('platform - fails deployment fast if deployment id failed', async t => {
  const provider = stubProvider();

  const getDeploymentResponse = sinon.stub().returns({
    status: 'failed',
    txHash: '0x2',
  });

  const fakeDeployment: Deployment & RemoteDeploymentId = {
    address: '0x1aec6468218510f19bb19f52c4767996895ce711',
    txHash: '0xc48e21ac9c051922f5ccf1b47b62000f567ef9bbc108d274848b44351a6872cb',
    remoteDeploymentId: 'abc',
  };

  const deployment = await resumeOrDeploy(
    provider,
    fakeDeployment,
    provider.deployPending,
    undefined,
    undefined,
    undefined,
    undefined,
    getDeploymentResponse,
  );
  t.true(getDeploymentResponse.calledOnceWithExactly('abc'));
  await t.throwsAsync(waitAndValidateDeployment(provider, deployment, undefined, undefined, getDeploymentResponse));
});

test('platform - deployment id timeout - no params', async t => {
  const fakeDeployment: Deployment & RemoteDeploymentId = {
    address: '0x1aec6468218510f19bb19f52c4767996895ce711',
    txHash: '0xc48e21ac9c051922f5ccf1b47b62000f567ef9bbc108d274848b44351a6872cb',
    remoteDeploymentId: 'abc',
  };
  try {
    throw new TransactionMinedTimeout(fakeDeployment);
  } catch (e: any) {
    const EXPECTED =
      /Timed out waiting for contract deployment to address \S+ with deployment id abc\n\nRun the function again to continue waiting for the transaction confirmation./;
    t.true(EXPECTED.test(e.message), e.message);
  }
});

test('platform - deployment id timeout - params', async t => {
  const fakeDeployment: Deployment & RemoteDeploymentId = {
    address: '0x1aec6468218510f19bb19f52c4767996895ce711',
    txHash: '0xc48e21ac9c051922f5ccf1b47b62000f567ef9bbc108d274848b44351a6872cb',
    remoteDeploymentId: 'abc',
  };
  try {
    throw new TransactionMinedTimeout(fakeDeployment, 'implementation', true);
  } catch (e: any) {
    const EXPECTED =
      /Timed out waiting for implementation contract deployment to address \S+ with deployment id abc+\n\nRun the function again to continue waiting for the transaction confirmation. If the problem persists, adjust the polling parameters with the timeout and pollingInterval options./;
    t.true(EXPECTED.test(e.message), e.message);
  }
});
