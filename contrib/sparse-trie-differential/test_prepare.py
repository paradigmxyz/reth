#!/usr/bin/env python3
"""Check preparation and its exact compiler wrapper without compiling Rust."""
import hashlib
import json
import os
import pathlib
import runpy
import shutil
import subprocess
import sys
import tempfile
import tomllib
import unittest
from unittest.mock import patch


class PrepareTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.wrapper = pathlib.Path(self.temporary.name) / 'wrapper.py'
        prepare = pathlib.Path(__file__).with_name('prepare.sh').read_text()
        self.wrapper.write_text(prepare.split("<<'PYWRAPPER'\n", 1)[1].split('\nPYWRAPPER', 1)[0])

    def invoke(self, args):
        with patch.object(sys, 'argv', [str(self.wrapper), '/fake/rustc', *args]), \
                patch('os.execvp') as execute:
            runpy.run_path(str(self.wrapper), run_name='__main__')
            return execute.call_args.args

    def test_sparse_buckets_and_native_instrumentation_are_target_only(self):
        flags = ['--cfg', 'reth_sparse_trie_coverage',
                 '--check-cfg=cfg(reth_sparse_trie_coverage)',
                 '-Cpasses=sancov-module', '-Cllvm-args=-sanitizer-coverage-level=3',
                 '-Cllvm-args=-sanitizer-coverage-inline-8bit-counters']
        for crate in [['--crate-name', 'reth_trie_sparse'], ['--crate-name=reth_trie_sparse']]:
            for feature in [['--cfg', 'feature="fuzz"'], ['--cfg=feature="fuzz"']]:
                args = [*crate, *feature, '--out-dir', '/a/path-with-sancov',
                        '-C', 'opt-level=3', 'input with spaces.rs']
                self.assertEqual(self.invoke(args), ('/fake/rustc', ['/fake/rustc', *args, *flags]))
        for args in [['--crate-name', 'serde', '--cfg', 'feature="std"'], ['-vV']]:
            self.assertEqual(self.invoke(args), ('/fake/rustc', ['/fake/rustc', *args]))

    def test_missing_fuzz_feature_is_rejected(self):
        with self.assertRaises(SystemExit) as error:
            self.invoke(['--crate-name', 'reth_trie_sparse'])
        self.assertEqual(str(error.exception), 'reth_trie_sparse must enable the fuzz feature')

    def test_adapter_hashes_actual_dependency_without_instrumenting_itself(self):
        target = pathlib.Path(self.temporary.name) / 'target.rlib'
        target.write_bytes(b'actual target build')
        for extern in [['--extern', f'reth_trie_sparse={target}'],
                       [f'--extern=reth_trie_sparse={target}']]:
            args = ['--crate-name', 'sparse_trie_differential_workload', *extern]
            with patch.dict(os.environ):
                self.assertEqual(self.invoke(args), ('/fake/rustc', ['/fake/rustc', *args]))
                self.assertEqual(os.environ['SPARSE_TRIE_NATIVE_COVERAGE_ID'],
                                 hashlib.sha256(target.read_bytes()).hexdigest())

    def test_external_instrumentation_and_missing_dependency_are_rejected(self):
        cases = [
            (['-Cpasses=sancov-module'],
             'external SanCov flags would instrument the adapter; remove them'),
            (['-C', 'llvm-args=-sanitizer-coverage-level=3'],
             'external SanCov flags would instrument the adapter; remove them'),
            (['--codegen=passes=sancov-module'],
             'external SanCov flags would instrument the adapter; remove them'),
            (['--codegen', 'llvm-args=-sanitizer-coverage-level=3'],
             'external SanCov flags would instrument the adapter; remove them'),
            ([], 'adapter is missing the reth_trie_sparse dependency'),
        ]
        for flags, message in cases:
            with self.assertRaises(SystemExit) as error:
                self.invoke(['--crate-name', 'sparse_trie_differential_workload', *flags])
            self.assertEqual(str(error.exception), message)

    def test_fuzz_profile_keeps_checks_symbols_and_distinct_buckets(self):
        manifest = tomllib.loads(pathlib.Path(__file__).with_name('Cargo.toml').read_text())
        self.assertEqual(manifest['profile']['fuzz'], {
            'inherits': 'release', 'debug': 'full', 'strip': 'none',
            'debug-assertions': True, 'overflow-checks': True,
            'panic': 'unwind', 'lto': False,
        })

    def test_prepare_builds_copies_and_records_fuzz_profile(self):
        temporary = pathlib.Path(self.temporary.name)
        root = temporary / 'source'
        workload = root / 'contrib' / 'sparse-trie-differential'
        workload.mkdir(parents=True)
        for name in ('prepare.sh', 'Cargo.toml'):
            shutil.copy2(pathlib.Path(__file__).with_name(name), workload / name)
        # Exercise preparation end to end; only external tool outputs and the native smoke
        # are synthetic. No Rust compilation or native library loading takes place.
        (workload / 'smoke.py').write_text(
            'import pathlib, sys\n'
            'assert pathlib.Path(sys.argv[1]).read_bytes() == b"fuzz-library"\n')
        tools = temporary / 'fake tools'
        tools.mkdir()
        stub = tools / 'stub.py'
        stub.write_text('''#!/usr/bin/env python3
import json, os, pathlib, sys
name = pathlib.Path(sys.argv[0]).name
if name == 'rustup':
    print(pathlib.Path(__file__).with_name(sys.argv[2]))
elif name in ('cargo', 'rustc'):
    if sys.argv[1:] == ['--version']:
        print(name + ' test-toolchain')
    else:
        pathlib.Path(os.environ['CARGO_ARGS']).write_text(json.dumps(sys.argv[1:]))
        library = pathlib.Path(os.environ['CARGO_TARGET_DIR']) / 'fuzz' / 'libsparse_trie_differential_workload.so'
        library.parent.mkdir(parents=True)
        library.write_bytes(b'fuzz-library')
elif name == 'git':
    if sys.argv[3] == 'ls-files':
        sys.stdout.write('contrib/sparse-trie-differential/Cargo.toml\\0'
                         'contrib/sparse-trie-differential/prepare.sh\\0'
                         'contrib/sparse-trie-differential/smoke.py\\0')
    elif sys.argv[3:] == ['rev-parse', 'HEAD']:
        print('a' * 40)
    elif sys.argv[3:] != ['status', '--porcelain']:
        raise SystemExit('unexpected git invocation: ' + repr(sys.argv))
elif name == 'nm':
    for backend in ('arena', 'arena_persistent'):
        for index in range(28674):
            print(f'reth_trie_sparse::{backend}::sparse_guidance::coverage_arm::<{index}>')
elif name != 'ldd':
    raise SystemExit('unexpected tool: ' + name)
''')
        for name in ('cargo', 'rustc', 'rustup', 'git', 'nm', 'ldd'):
            executable = tools / name
            shutil.copy2(stub, executable)
            executable.chmod(0o755)
        config = temporary / 'config.json'
        config.write_text('{}')
        args_file = temporary / 'cargo-args.json'
        run_dir = temporary / 'prepared'
        target_dir = temporary / 'target'
        environment = dict(os.environ, PATH=str(tools) + os.pathsep + os.environ['PATH'],
                           RUN_DIR=str(run_dir), CARGO_TARGET_DIR=str(target_dir),
                           CARGO_ARGS=str(args_file))
        for name in ('RUSTC', 'RUSTC_WRAPPER', 'RUSTC_WORKSPACE_WRAPPER'):
            environment.pop(name, None)
        result = subprocess.run(['bash', str(workload / 'prepare.sh'),
                                 '--workload-config', str(config)], env=environment,
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(json.loads(args_file.read_text()), [
            'rustc', '--locked', '--profile', 'fuzz', '--manifest-path',
            str(workload / 'Cargo.toml'), '--lib', '--',
            '-C', 'link-arg=-Wl,-rpath,$ORIGIN/lib', '-C', 'link-arg=-Wl,-Bsymbolic-functions',
        ])
        self.assertFalse((target_dir / 'release').exists())
        bundle = run_dir / 'execution'
        self.assertEqual((bundle / 'libworkload.so').read_bytes(), b'fuzz-library')
        self.assertEqual(json.loads((bundle / 'build-info.json').read_text())['profile'], 'fuzz')
        self.assertEqual(json.loads((run_dir / 'fuzz-session.json').read_text()),
                         {'version': 2, 'execution_bundle': 'execution'})


if __name__ == '__main__':
    unittest.main()
