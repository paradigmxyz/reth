const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const { test } = require('node:test');

for (const filename of ['bench.yml', 'bench-scheduled.yml']) {
  const workflow = fs.readFileSync(path.join(__dirname, '../workflows', filename), 'utf8');
  const source = workflow.match(/          # Keep the candidate's LLVM version[\s\S]+?(?=\n\n          # uv)/)[0]
    .replace(/^ {10}/gm, '');

  test(`${filename}: workflow dependencies are available in every benchmark mode`, () => {
    const checkout = workflow.match(/      - name: Check out dependencies from workflow revision[\s\S]+?(?=\n      - name:)/)[0];
    assert.doesNotMatch(checkout, /^\s+if:/m);
    assert.match(checkout, /ref: \$\{\{ github.workflow_sha \}\}/);
    assert.match(checkout, /path: \.bench-workflow\n/);
    assert.match(checkout, /sparse-checkout: \|\n            \.github\/scripts\n            scripts\/bench-reth-charts\n/);
    assert.doesNotMatch(workflow, /\.bench-chart-workflow/);
  });

  for (const version of ['22', '23', 'invalid']) {
    test(`${filename}: candidate LLVM ${version} uses the workflow installer or fails closed`, t => {
      const scratch = fs.mkdtempSync(path.join(os.tmpdir(), 'bench-llvm-revision-'));
      t.after(() => fs.rmSync(scratch, { recursive: true, force: true }));
      const candidateScripts = path.join(scratch, '.github/scripts');
      const workflowScripts = path.join(scratch, '.bench-workflow/.github/scripts');
      const bin = path.join(scratch, 'bin');
      for (const directory of [candidateScripts, workflowScripts, bin]) {
        fs.mkdirSync(directory, { recursive: true });
      }
      // Executing the historical installer would fail instead of reaching our sudo stub.
      const candidate = `#!/usr/bin/env bash\nversion=\${2:-${version}}\nexit 99\n`;
      const candidatePath = path.join(candidateScripts, 'install_llvm.sh');
      fs.writeFileSync(candidatePath, candidate, { mode: 0o755 });
      for (const script of ['install_llvm.sh', 'install_llvm_ubuntu.sh']) {
        fs.copyFileSync(path.join(__dirname, script), path.join(workflowScripts, script));
        fs.chmodSync(path.join(workflowScripts, script), 0o755);
      }
      fs.writeFileSync(path.join(bin, 'sudo'), '#!/usr/bin/env bash\nprintf "%s\\n" "$@" > "$LLVM_TEST_LOG"\n', { mode: 0o755 });
      fs.writeFileSync(path.join(bin, 'llvm-config'), '#!/usr/bin/env bash\ncase "$1" in --prefix) echo /test/llvm;; --version) echo 23.1.0;; esac\n', { mode: 0o755 });
      const log = path.join(scratch, 'install-args');
      const result = spawnSync('bash', ['-eo', 'pipefail', '-c', source], {
        cwd: scratch,
        encoding: 'utf8',
        env: { ...process.env, GITHUB_WORKSPACE: scratch, GITHUB_ENV: path.join(scratch, 'env'),
          LLVM_TEST_LOG: log, PATH: `${bin}:${process.env.PATH}` },
      });
      assert.equal(result.status, version === 'invalid' ? 1 : 0, result.stderr);
      if (version === 'invalid') {
        assert.equal(fs.existsSync(log), false);
        assert.equal(result.stdout.trim(), "::error::Cannot determine the candidate's LLVM version");
      } else {
        assert.equal(fs.readFileSync(log, 'utf8'), `${workflowScripts}/install_llvm_ubuntu.sh\n${version}\n`);
      }
      assert.equal(fs.readFileSync(candidatePath, 'utf8'), candidate);
    });
  }
}
