const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { test } = require('node:test');
const { uploadCharts } = require('./bench-upload-charts');
const { readState } = require('./bench-read-state');

const png = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII=', 'base64');
const asset = 'https://github.com/user-attachments/assets/12345678-abcd-1234-abcd-123456789abc';
const repo = 'owner/repo';
function fixture(t, files = ['latency_throughput.png']) {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'bench-upload-test-'));
  t.after(() => fs.rmSync(directory, { recursive: true, force: true }));
  for (const file of files) fs.writeFileSync(path.join(directory, file), png);
  return directory;
}
function apiFixture() {
  const uploads = [];
  const api = args => {
    if (args[0] === 'repos/' + repo) return JSON.stringify({ id: 123, permissions: { push: true } });
    uploads.push(args);
    return JSON.stringify({ url: asset });
  };
  return { api, uploads };
}

test('uploads raw PNG bytes with gh attachment query fields; renders reusable Markdown', t => {
  const directory = fixture(t);
  const { api, uploads } = apiFixture();
  const markdown = uploadCharts(repo, directory, api);
  assert.equal(uploads.length, 1);
  assert.deepEqual(uploads[0], [
    'https://uploads.github.com/user-attachments/assets',
    '--method', 'POST',
    '-H', 'Content-Type: application/octet-stream',
    '-H', 'Accept: application/vnd.github+json',
    '--input', path.join(directory, 'latency_throughput.png'),
    '-f', 'name=latency_throughput.png',
    '-f', 'content_type=image/png',
    '-F', 'repository_id=123',
  ]);
  assert.deepEqual(fs.readFileSync(uploads[0][8]), png);
  assert.ok(markdown.includes('![Latency, Throughput & Diff](' + asset + ')'));
  assert.equal(fs.readFileSync(path.join(directory, 'charts.md'), 'utf8'), markdown);
  assert.equal(JSON.parse(fs.readFileSync(path.join(directory, 'attachments.json'))).repositoryId, 123);
});

test('validates the whole batch before uploading', t => {
  const directory = fixture(t, ['a.png', 'z.png']);
  fs.writeFileSync(path.join(directory, 'z.png'), 'not an image');
  assert.throws(() => uploadCharts(repo, directory, () => assert.fail('No API calls expected')), /PNG signature/);
});

test('rejects empty, oversized and missing charts before uploading', t => {
  const directory = fixture(t);
  fs.truncateSync(path.join(directory, 'latency_throughput.png'), 0);
  assert.throws(() => uploadCharts(repo, directory), /nonempty/);
  fs.truncateSync(path.join(directory, 'latency_throughput.png'), 10 * 1024 * 1024 + 1);
  assert.throws(() => uploadCharts(repo, directory), /10 MiB/);
  fs.unlinkSync(path.join(directory, 'latency_throughput.png'));
  assert.throws(() => uploadCharts(repo, directory), /No PNG/);
});

test('requires write access before POST', t => {
  const directory = fixture(t);
  assert.throws(() => uploadCharts(repo, directory, args => {
    assert.equal(args[0], 'repos/' + repo);
    return JSON.stringify({ id: 123, permissions: { push: false } });
  }), /write access/);
});

test('reuses successful uploads after a partial failure without retrying POST', t => {
  const directory = fixture(t, ['a.png', 'b.png']);
  const { api, uploads } = apiFixture();
  assert.throws(() => uploadCharts(repo, directory, args => {
    if (args.includes('name=b.png')) throw new Error('HTTP 429');
    return api(args);
  }), /429/);
  assert.equal(uploads.length, 1);
  assert.equal(fs.existsSync(path.join(directory, 'charts.md')), false);
  uploadCharts(repo, directory, api);
  assert.equal(uploads.length, 2);
  assert.ok(uploads[1].includes('name=b.png'));
  uploadCharts(repo, directory, api);
  assert.equal(uploads.length, 2);
});

test('changed bytes or repository cause a fresh upload', t => {
  const directory = fixture(t);
  const { api, uploads } = apiFixture();
  uploadCharts(repo, directory, api);
  fs.appendFileSync(path.join(directory, 'latency_throughput.png'), '\n');
  uploadCharts(repo, directory, api);
  assert.equal(uploads.length, 2);
  uploadCharts(repo, directory, args => args[0] === 'repos/' + repo
    ? JSON.stringify({ id: 456, permissions: { push: true } }) : api(args));
  assert.equal(uploads.length, 3);
  assert.ok(uploads[2].includes('repository_id=456'));
});

test('refuses invalid response URLs and preserves prior manifest successes', t => {
  const directory = fixture(t, ['a.png', 'b.png']);
  const { api } = apiFixture();
  assert.throws(() => uploadCharts(repo, directory, args =>
    args.includes('name=b.png') ? JSON.stringify({ url: 'https://example.com/wrong' }) : api(args)),
  /no valid attachment URL/);
  const saved = JSON.parse(fs.readFileSync(path.join(directory, 'attachments.json')));
  assert.equal(saved.files['a.png'].url, asset);
  assert.equal(saved.files['b.png'], undefined);
});

const sha = 'a'.repeat(40);
const workflow = 'bench-scheduled.yml';
const artifact = (id, branch = 'main') => ({
  id, expired: false, workflow_run: { id, head_branch: branch },
});
const run = overrides => ({
  conclusion: 'success', head_branch: 'main', event: 'schedule',
  path: '.github/workflows/' + workflow, ...overrides,
});
function stateApi(artifacts, runs = {}) {
  return endpoint => {
    if (endpoint === 'repos/' + repo) return { default_branch: 'main' };
    if (endpoint.includes('/actions/artifacts?')) return { artifacts };
    const id = endpoint.split('/').at(-1);
    assert.ok(runs[id], endpoint);
    return runs[id];
  };
}

test('first run or expired state bootstraps without a download', () => {
  assert.equal(readState(repo, 'nightly-last-feature-ref', workflow,
    stateApi([]), () => assert.fail('unexpected download')), '');
  assert.equal(readState(repo, 'nightly-last-feature-ref', workflow,
    stateApi([{ ...artifact(1), expired: true }]), () => assert.fail('unexpected download')), '');
});

test('ignores PR, wrong workflow, pending and failed artifacts; selects latest successful state', () => {
  const artifacts = [artifact(6, 'feature'), artifact(5), artifact(4), artifact(3), artifact(2), artifact(1)];
  const api = stateApi(artifacts, {
    5: run({ event: 'pull_request_target' }),
    4: run({ path: '.github/workflows/other.yml' }),
    3: run({ conclusion: null }),
    2: run({ conclusion: 'failure' }),
    1: run({}),
  });
  assert.equal(readState(repo, 'nightly-last-feature-ref', workflow, api, (repository, id) => {
    assert.equal(repository, repo);
    assert.equal(id, 1);
    return sha + '\n';
  }), sha);
});

test('paginates artifact lookup', () => {
  let pages = 0;
  const api = endpoint => {
    if (endpoint === 'repos/' + repo) return { default_branch: 'main' };
    if (endpoint.includes('/actions/artifacts?')) {
      pages++;
      assert.ok(endpoint.includes('page=' + pages));
      return { artifacts: pages === 1
        ? Array.from({ length: 100 }, (_, id) => ({ ...artifact(id), expired: true }))
        : [artifact(101)] };
    }
    return run({});
  };
  assert.equal(readState(repo, 'nightly-last-feature-ref', workflow, api, () => sha), sha);
  assert.equal(pages, 2);
});

test('propagates API failures rather than silently resetting the baseline', () => {
  assert.throws(() => readState(repo, 'nightly-last-feature-ref', workflow,
    () => { throw new Error('HTTP 401'); }), /401/);
});

test('rejects corrupt state', () => {
  assert.throws(() => readState(repo, 'nightly-last-feature-ref', workflow,
    stateApi([artifact(1)], { 1: run({}) }), () => 'not a SHA'), /Invalid commit SHA/);
});

// Opt-in live check: uploads one synthetic image, does not create a comment.
// Run with a user PAT/OAuth token and repository write access.
test('live GitHub image attachment', { skip: !process.env.BENCH_LIVE_UPLOAD_REPO }, t => {
  const directory = fixture(t);
  const markdown = uploadCharts(process.env.BENCH_LIVE_UPLOAD_REPO, directory);
  console.log(markdown);
  assert.ok(markdown.includes('https://github.com/user-attachments/assets/'));
});
