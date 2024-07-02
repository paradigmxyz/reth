import test from 'ava';
import { EthereumProvider, getTransactionReceipt } from './provider';

const hash = '0x1234';

function makeProviderReturning(result: unknown): EthereumProvider {
  return { send: (_method: string, _params: unknown[]) => Promise.resolve(result) } as EthereumProvider;
}

test('getTransactionReceipt returns null', async t => {
  const provider = makeProviderReturning(null);
  t.is(await getTransactionReceipt(provider, hash), null);
});

test('getTransactionReceipt status returns 0x0', async t => {
  const provider = makeProviderReturning({ status: '0x0' });
  const receipt = await getTransactionReceipt(provider, hash);
  t.is(receipt?.status, '0x0');
});

test('getTransactionReceipt status returns 0x1', async t => {
  const provider = makeProviderReturning({ status: '0x1' });
  const receipt = await getTransactionReceipt(provider, hash);
  t.is(receipt?.status, '0x1');
});

test('getTransactionReceipt status normalizes to 0x0', async t => {
  const provider = makeProviderReturning({ status: '0x000000000000000000000000' });
  const receipt = await getTransactionReceipt(provider, hash);
  t.is(receipt?.status, '0x0');
});

test('getTransactionReceipt status normalizes to 0x1', async t => {
  const provider = makeProviderReturning({ status: '0x000000000000000000000001' });
  const receipt = await getTransactionReceipt(provider, hash);
  t.is(receipt?.status, '0x1');
});

test('getTransactionReceipt status returns empty hex', async t => {
  const provider = makeProviderReturning({ status: '0x' });
  const receipt = await getTransactionReceipt(provider, hash);
  t.is(receipt?.status, '0x');
});
