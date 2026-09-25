import assert from 'node:assert/strict';
import { readFile, readdir } from 'node:fs/promises';
import { setTimeout as sleep } from 'node:timers/promises';
import Ajv from 'ajv';
import { load } from 'js-yaml';
import { SigningKey, computeAddress, concat, encodeRlp, keccak256, toBeHex } from 'ethers';

const endpoint = process.env.RPC_URL ?? 'http://localhost:8545';
const schemaDir = process.env.SCHEMA_DIR ?? '/spec/src/schemas';
// Public disposable key, funded only by this test genesis.
const key = new SigningKey(toBeHex(1, 32));
const sender = computeAddress(key.publicKey).toLowerCase();
const storage = '0x0000000000000000000000000000000000001000';
const reverter = '0x0000000000000000000000000000000000001001';
const readOne = toBeHex(1, 32);
let requestId = 0;

class RpcError extends Error {
  constructor(method, error) {
    super(`${method}: ${JSON.stringify(error)}`);
    this.code = error.code;
  }
}

async function rpc(method, params = []) {
  const response = await fetch(endpoint, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ jsonrpc: '2.0', id: ++requestId, method, params }),
    signal: AbortSignal.timeout(30_000),
  });
  assert.equal(response.ok, true, `HTTP ${response.status}`);
  const body = await response.json();
  if (body.error) throw new RpcError(method, body.error);
  return body.result;
}

async function waitFor(label, read, timeout = 60_000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    const result = await read();
    if (result) return result;
    await sleep(500);
  }
  throw new Error(`Timed out waiting for ${label}`);
}

const schemas = {};
for (const name of (await readdir(schemaDir)).filter((name) => name.endsWith('.yaml'))) {
  Object.assign(schemas, load(await readFile(`${schemaDir}/${name}`, 'utf8')));
}
const ajv = new Ajv({ strict: false, allErrors: true });
ajv.addSchema({ $id: 'execution-apis', components: { schemas } });
function validate(name, value) {
  const check = ajv.getSchema(`execution-apis#/components/schemas/${name}`);
  assert(check, `Missing schema ${name}`);
  assert(check(value), `${name}: ${ajv.errorsText(check.errors, { separator: '\n' })}\n${JSON.stringify(value)}`);
}

// EIP-8141 integers use minimal RLP bytes; JSON-RPC quantities use hex strings.
const integer = (value) => BigInt(value) === 0n ? '0x' : toBeHex(BigInt(value));
function encode(tx) {
  return concat(['0x06', encodeRlp([
    integer(tx.chainId), integer(tx.nonce), tx.from,
    tx.frames.map((frame) => [
      integer(frame.mode), integer(frame.flags), frame.target ?? '0x',
      [integer(frame.executionGas), integer(frame.stateGas)], integer(frame.value), frame.data,
    ]),
    tx.signatures.map((sig) => [integer(sig.scheme), sig.signer ?? '0x', sig.msg ?? '0x', sig.signature ?? '0x']),
    [integer(tx.maxPriorityFeePerGas), integer(tx.maxFeePerGas), integer(tx.maxFeePerBlobGas)],
    tx.blobVersionedHashes,
  ])]);
}
function sign(unsigned, signingKey = key) {
  const tx = structuredClone(unsigned);
  assert.equal(tx.signatures.length, 1);
  assert.equal(tx.signatures[0].msg ?? '0x', '0x');
  const signature = signingKey.sign(keccak256(encode(tx)));
  tx.signatures[0].signature = concat([toBeHex(signature.yParity, 1), signature.r, signature.s]);
  return tx;
}

const verify = (limits = {}) => ({ mode: '0x1', flags: '0x3', ...limits });
const request = (frames) => ({
  type: '0x6', from: sender, frames, signatures: [{ scheme: '0x1' }],
  maxPriorityFeePerGas: '0x3b9aca00', maxFeePerGas: '0x2540be400', maxFeePerBlobGas: '0x0',
});

async function fill(frames) {
  const result = await rpc('eth_fillTransaction', [request(frames)]);
  validate('Transaction8141Unsigned', result.tx);
  assert.equal(result.raw, encode(result.tx), 'Filled raw envelope differs from its JSON fields');
  for (const [index, frame] of frames.entries()) {
    for (const field of ['executionGas', 'stateGas']) {
      if (field in frame) assert.equal(result.tx.frames[index][field], frame[field], `Changed explicit ${field}`);
    }
  }
  return result.tx;
}

async function submit(name, unsigned, statuses, status = '0x1') {
  const before = BigInt(await rpc('eth_getBalance', [sender, 'latest']));
  const signed = sign(unsigned);
  validate('Transaction8141', signed);
  const raw = encode(signed);
  const hash = await rpc('eth_sendRawTransaction', [raw]);
  assert.equal(hash, keccak256(raw));
  const receipt = await waitFor(`${name} receipt`, () => rpc('eth_getTransactionReceipt', [hash]));
  const transaction = await rpc('eth_getTransactionByHash', [hash]);
  validate('TransactionInfo', transaction);
  validate('ReceiptInfo', receipt);
  assert.equal(receipt.payer, sender);
  assert.equal(receipt.from, sender);
  assert.equal(receipt.status, status);
  assert.deepEqual(transaction.frames, signed.frames);
  assert.deepEqual(transaction.signatures, signed.signatures);
  assert.deepEqual(receipt.frameReceipts.map((frame) => frame.status), statuses);
  for (const frame of receipt.frameReceipts) {
    assert.equal(BigInt(frame.gasUsed), BigInt(frame.executionGasUsed) + BigInt(frame.stateGasUsed));
    if (frame.status !== '0x1') {
      assert.equal(BigInt(frame.stateGasUsed), 0n);
      assert.deepEqual(frame.logs, []);
    }
  }
  const after = BigInt(await rpc('eth_getBalance', [sender, receipt.blockNumber]));
  assert.equal(before - after, BigInt(receipt.gasUsed) * BigInt(receipt.effectiveGasPrice));
  console.log(`PASS ${name}: ${hash}, gasUsed=${BigInt(receipt.gasUsed)}`);
  return receipt;
}

await waitFor('RPC readiness', async () => {
  try { return await rpc('web3_clientVersion'); } catch { return false; }
}, 120_000);
assert.equal(BigInt(await rpc('eth_chainId')), 8141n);
assert.equal(BigInt(await rpc('eth_getTransactionCount', [sender, 'latest'])), 0n, 'Use a fresh test chain');

const single = await fill([verify({ stateGas: '0x0' })]);
assert(BigInt(single.frames[0].executionGas) > 0n);
await submit('single frame and explicit zero state gas', single, ['0x1']);

const shared = await fill([verify(), { mode: '0x2', target: storage }, { mode: '0x2', target: storage, data: readOne }]);
assert(BigInt(shared.frames[1].stateGas) > 0n);
const sharedReceipt = await submit('write then read across frames', shared, ['0x1', '0x1', '0x1']);
assert.equal(BigInt(await rpc('eth_getStorageAt', [storage, '0x0', sharedReceipt.blockNumber])), 1n);
assert(BigInt(sharedReceipt.frameReceipts[1].stateGasUsed) > 0n);

// Intentional reverts need explicit limits because gas filling searches for successful execution.
const limits = { executionGas: '0x30d40', stateGas: '0xf4240' };
const reverted = await fill([
  verify({ executionGas: '0xc350', stateGas: '0x0' }),
  { mode: '0x2', target: reverter, ...limits },
  { mode: '0x2', target: storage, data: readOne, ...limits },
]);
const revertReceipt = await submit('reverting frame rolls back and execution continues', reverted, ['0x1', '0x0', '0x1'], '0x0');
assert.equal(BigInt(await rpc('eth_getStorageAt', [reverter, '0x0', revertReceipt.blockNumber])), 0n);

await assert.rejects(() => fill([verify({ executionGas: '0x0' })]), (error) => {
  assert(error instanceof RpcError);
  assert.match(error.message, /gas|VERIFY/i);
  console.log(`Zero-limit rejection: ${error.message}`);
  return true;
});
console.log('PASS explicit zero execution gas is not filled');

const invalid = await fill([verify()]);
invalid.signatures[0].signature = `0x${'00'.repeat(65)}`;
await assert.rejects(() => rpc('eth_sendRawTransaction', [encode(invalid)]), (error) => {
  assert(error instanceof RpcError);
  assert.match(error.message, /signature/i);
  console.log(`Signature rejection: ${error.message}`);
  return true;
});
const wrongSigner = sign(await fill([verify()]), new SigningKey(toBeHex(2, 32)));
await assert.rejects(() => rpc('eth_sendRawTransaction', [encode(wrongSigner)]), (error) => {
  assert(error instanceof RpcError);
  assert.match(error.message, /validation prefix execution failed/);
  console.log(`Wrong-signer rejection: ${error.message}`);
  return true;
});
assert.equal(BigInt(await rpc('eth_getTransactionCount', [sender, 'pending'])), 3n);
console.log('PASS nonempty invalid signature rejected');
console.log('PASS all frame RPC devnet checks');
