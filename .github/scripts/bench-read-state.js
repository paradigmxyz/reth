#!/usr/bin/env node
// Scheduler state lives in an Actions artifact uploaded only after a successful
// benchmark. Expiry (90 days) or first use returns empty and bootstraps a baseline.
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { execFileSync } = require('node:child_process');

function gh(endpoint) {
  return JSON.parse(execFileSync('gh', ['api', endpoint], {
    encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'],
  }));
}

function downloadState(repo, id) {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'bench-state-'));
  try {
    const archive = path.join(directory, 'state.zip');
    fs.writeFileSync(archive, execFileSync('gh', [
      'api', 'repos/' + repo + '/actions/artifacts/' + id + '/zip',
    ], { maxBuffer: 1024 * 1024, stdio: ['ignore', 'pipe', 'pipe'] }));
    // Read only the expected member; never extract arbitrary archive paths.
    return execFileSync('unzip', ['-p', archive, 'last-feature-ref'], { encoding: 'utf8' }).trim();
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
}

function readState(repo, name, workflow, api = gh, download = downloadState) {
  if (!/^[\w.-]+\/[\w.-]+$/.test(repo)) throw new Error('Expected an owner/repository');
  const branch = api('repos/' + repo).default_branch;
  if (!branch) throw new Error('Could not determine default branch');
  for (let page = 1; ; page++) {
    const response = api('repos/' + repo + '/actions/artifacts?name=' +
      encodeURIComponent(name) + '&per_page=100&page=' + page);
    if (!Array.isArray(response.artifacts)) throw new Error('Invalid artifact listing');
    for (const artifact of response.artifacts) {
      if (artifact.expired || artifact.workflow_run?.head_branch !== branch) continue;
      const run = api('repos/' + repo + '/actions/runs/' + artifact.workflow_run.id);
      // Ignore similarly named artifacts from PRs, other workflows, and runs
      // that have not finished successfully.
      if (run.conclusion !== 'success' || run.head_branch !== branch ||
          !['schedule', 'workflow_dispatch'].includes(run.event) ||
          run.path?.split('@')[0] !== '.github/workflows/' + workflow) continue;
      const ref = download(repo, artifact.id).trim();
      if (!/^[a-f0-9]{40}$/.test(ref)) throw new Error('Invalid commit SHA in benchmark state artifact');
      return ref;
    }
    if (response.artifacts.length < 100) return '';
  }
}

module.exports = { readState };
if (require.main === module) {
  try {
    if (process.argv.length !== 5) throw new Error('Usage: bench-read-state.js owner/repo artifact-name workflow.yml');
    console.log(readState(...process.argv.slice(2)));
  } catch (error) {
    console.error(error.stderr?.toString().trim() || error.message);
    process.exitCode = 1;
  }
}
