#!/usr/bin/env node
// Uses the same upload API as gh --attach (gh v2.99.0):
// https://github.com/cli/cli/blob/v2.99.0/internal/attachments/client.go
// Requires a user PAT/OAuth token with write access, not an Actions installation token.
const fs = require('node:fs');
const path = require('node:path');
const crypto = require('node:crypto');
const { execFileSync } = require('node:child_process');

const labels = {
  'latency_throughput.png': 'Latency, Throughput & Diff',
  'wait_breakdown.png': 'Wait Time Breakdown',
  'gas_vs_latency.png': 'Gas vs Latency',
};

function gh(args) {
  return execFileSync('gh', ['api', ...args], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] });
}

function uploadCharts(repo, directory, api = gh) {
  if (!/^[\w.-]+\/[\w.-]+$/.test(repo)) throw new Error('Expected an owner/repository');
  const files = fs.readdirSync(directory).filter(file => file.endsWith('.png')).sort();
  if (files.length === 0) throw new Error('No PNG charts found');
  // Validate every file before starting any uploads.
  const charts = files.map(file => {
    const filename = path.join(directory, file);
    const stat = fs.statSync(filename);
    if (!stat.isFile() || stat.size === 0 || stat.size > 10 * 1024 * 1024) {
      throw new Error(file + ': expected a nonempty regular PNG file at most 10 MiB');
    }
    const bytes = fs.readFileSync(filename);
    if (!bytes.subarray(0, 8).equals(Buffer.from('89504e470d0a1a0a', 'hex'))) {
      throw new Error(file + ': invalid PNG signature');
    }
    return { file, filename, digest: crypto.createHash('sha256').update(bytes).digest('hex') };
  });
  const repository = JSON.parse(api(['repos/' + repo]));
  if (!Number.isSafeInteger(repository.id) || repository.id <= 0 || !repository.permissions?.push) {
    throw new Error('Attaching charts requires repository write access');
  }
  const manifestPath = path.join(directory, 'attachments.json');
  let manifest = { repositoryId: repository.id, files: {} };
  if (fs.existsSync(manifestPath)) {
    const saved = JSON.parse(fs.readFileSync(manifestPath, 'utf8'));
    if (saved.repositoryId === repository.id) manifest = saved;
  }
  const validUrl = value => typeof value === 'string' &&
    /^https:\/\/github\.com\/user-attachments\/assets\/[a-zA-Z0-9-]+$/.test(value);
  for (const chart of charts) {
    const previous = manifest.files[chart.file];
    if (previous?.digest === chart.digest && validUrl(previous.url)) continue;
    // Never retry POST automatically. Persist each successful URL immediately so
    // a rerun after a partial failure can reuse uploads that already succeeded.
    const result = JSON.parse(api([
      'https://uploads.github.com/user-attachments/assets',
      '--method', 'POST',
      '-H', 'Content-Type: application/octet-stream',
      '-H', 'Accept: application/vnd.github+json',
      '--input', chart.filename,
      '-f', 'name=' + chart.file,
      '-f', 'content_type=image/png',
      '-F', 'repository_id=' + repository.id,
    ]));
    if (!validUrl(result.url)) throw new Error(chart.file + ': upload returned no valid attachment URL');
    manifest.files[chart.file] = { digest: chart.digest, url: result.url };
    fs.writeFileSync(manifestPath, JSON.stringify(manifest, null, 2) + '\n');
  }
  let markdown = '\n\n### Charts\n\n';
  for (const chart of charts) {
    const label = labels[chart.file] || chart.file.replace(/[^\w .-]/g, '_');
    markdown += '<details><summary>' + label + '</summary>\n\n';
    markdown += '![' + label + '](' + manifest.files[chart.file].url + ')\n\n</details>\n\n';
  }
  fs.writeFileSync(path.join(directory, 'charts.md'), markdown);
  return markdown;
}

module.exports = { uploadCharts };
if (require.main === module) {
  try {
    if (process.argv.length !== 4) throw new Error('Usage: bench-upload-charts.js owner/repo charts-directory');
    uploadCharts(process.argv[2], process.argv[3]);
  } catch (error) {
    console.error(error.stderr?.toString().trim() || error.message);
    process.exitCode = 1;
  }
}
