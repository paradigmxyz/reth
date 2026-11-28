#!/usr/bin/env python3
"""Check preparation and its exact compiler wrapper without compiling Rust."""
import contextlib
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

    def invoke(self, args, cfg='debug_assertions\nreth_sparse_trie_coverage\nfeature="std"\n',
               rustc='/fake/rustc'):
        probe = (patch('subprocess.check_output', return_value=cfg)
                 if cfg is not None else contextlib.nullcontext())
        with patch.object(sys, 'argv', [str(self.wrapper), rustc, *args]), \
                patch('os.execvp') as execute, probe as query:
            runpy.run_path(str(self.wrapper), run_name='__main__')
            self.probe_args = query.call_args.args if cfg is not None and query.called else None
            return execute.call_args.args

    def test_sparse_buckets_and_native_instrumentation_are_target_only(self):
        flags = ['--cfg', 'reth_sparse_trie_coverage',
                 '--check-cfg=cfg(reth_sparse_trie_coverage)',
                 '-Cpasses=sancov-module', '-Cllvm-args=-sanitizer-coverage-level=3',
                 '-Cllvm-args=-sanitizer-coverage-trace-pc-guard']
        for crate in [['--crate-name', 'reth_trie_sparse'], ['--crate-name=reth_trie_sparse']]:
            for assertions in [['-C', 'debug-assertions=on'], ['-Cdebug-assertions=on'],
                               ['--codegen', 'debug-assertions=on'],
                               ['--codegen=debug-assertions=on']]:
                args = [*crate, *assertions, '--out-dir', '/a/path-with-sancov',
                        '-C', 'opt-level=3', 'input with spaces.rs']
                self.assertEqual(self.invoke(args), ('/fake/rustc', ['/fake/rustc', *args, *flags]))
                self.assertEqual(self.probe_args, (['/fake/rustc', *args, *flags, '--print=cfg'],))
                policy = json.loads(self.wrapper.with_name('target-build-policy.json').read_text())
                self.assertIs(policy['fuzz_feature'], False)
                self.assertIs(policy['debug_assertions'], True)
                self.assertIs(policy['orphan_node_checks'], True)
        for args in [['--crate-name', 'serde', '--cfg', 'feature="std"'], ['-vV']]:
            self.assertEqual(self.invoke(args), ('/fake/rustc', ['/fake/rustc', *args]))
            self.assertIsNone(self.probe_args)

    def test_fuzz_feature_and_spoofed_assertion_cfg_are_rejected(self):
        for flags, message in [
            (['--cfg', 'feature="fuzz"'], 'reth_trie_sparse must not enable the fuzz feature'),
            (['--cfg=feature="fuzz"'], 'reth_trie_sparse must not enable the fuzz feature'),
            (['--cfg', 'debug_assertions'],
             'use -C debug-assertions=on, not --cfg debug_assertions'),
            (['--cfg=debug_assertions'],
             'use -C debug-assertions=on, not --cfg debug_assertions'),
        ]:
            with self.assertRaises(SystemExit) as error:
                self.invoke(['--crate-name', 'reth_trie_sparse', *flags])
            self.assertEqual(str(error.exception), message)
        self.assertFalse(self.wrapper.with_name('target-build-policy.json').exists())

    def test_effective_cfg_must_have_assertions_coverage_and_no_fuzz(self):
        for cfg, message in [
            ('reth_sparse_trie_coverage\n',
             'reth_trie_sparse requires debug assertions (-C debug-assertions=on)'),
            ('debug_assertions\n', 'reth_trie_sparse requires reth_sparse_trie_coverage'),
            ('debug_assertions\nreth_sparse_trie_coverage\nfeature="fuzz"\n',
             'reth_trie_sparse must not enable the fuzz feature'),
        ]:
            with self.assertRaises(SystemExit) as error:
                self.invoke(['--crate-name', 'reth_trie_sparse', '-Cdebug-assertions=on'], cfg=cfg)
            self.assertEqual(str(error.exception), message)
        self.assertFalse(self.wrapper.with_name('target-build-policy.json').exists())

    def test_actual_rustc_debug_assertion_flags_and_overrides(self):
        rustc = shutil.which('rustc')
        if rustc is None:
            self.skipTest('rustc is unavailable for --print=cfg (no compilation)')
        for flags, enabled in [
            (['-C', 'opt-level=3', '-C', 'debug-assertions=on'], True),
            (['-Copt-level=3', '-Cdebug-assertions=off'], False),
            (['--codegen=debug-assertions=on'], True),
            (['--codegen', 'debug-assertions=off'], False),
            (['-Cdebug-assertions=on', '-Cdebug-assertions=off'], False),
            (['-Cdebug-assertions=off', '-Cdebug-assertions=on'], True),
        ]:
            with self.subTest(flags=flags):
                args = ['--crate-name', 'reth_trie_sparse', *flags]
                if enabled:
                    self.assertEqual(self.invoke(args, cfg=None, rustc=rustc)[0], rustc)
                else:
                    with self.assertRaises(SystemExit) as error:
                        self.invoke(args, cfg=None, rustc=rustc)
                    self.assertEqual(str(error.exception),
                                     'reth_trie_sparse requires debug assertions (-C debug-assertions=on)')

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
        self.assertEqual(manifest['dependencies']['reth-trie-sparse']['features'], ['std'])
        self.assertIn('rayon', manifest['dependencies'])
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
import json, os, pathlib, subprocess, sys
name = pathlib.Path(sys.argv[0]).name
if name == 'rustup':
    print(pathlib.Path(__file__).with_name(sys.argv[2]))
elif name == 'rustc':
    if sys.argv[1:] == ['--version']:
        print('rustc test-toolchain')
    elif '--print=cfg' in sys.argv:
        print('debug_assertions\\nfeature="std"\\nreth_sparse_trie_coverage')
    # Other rustc calls simulate a successful native compilation.
elif name == 'cargo':
    if sys.argv[1:] == ['--version']:
        print('cargo test-toolchain')
    else:
        pathlib.Path(os.environ['CARGO_ARGS']).write_text(json.dumps(sys.argv[1:]))
        if not os.environ.get('REUSE_UNCHECKED_TARGET'):
            subprocess.run([os.environ['RUSTC_WRAPPER'], os.environ['RUSTC'],
                            '--crate-name', 'reth_trie_sparse', '-C', 'debug-assertions=on'], check=True)
        library = pathlib.Path(os.environ['CARGO_TARGET_DIR']) / 'fuzz' / 'libsparse_trie_differential_workload.so'
        library.parent.mkdir(parents=True, exist_ok=True)
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
        provenance = json.loads((bundle / 'build-info.json').read_text())
        self.assertEqual(provenance['profile'], 'fuzz')
        coverage = provenance['coverage']
        self.assertEqual(coverage['kind'], 'llvm-sancov-trace-pc-guard')
        self.assertIs(coverage['fuzz_feature'], False)
        self.assertIs(coverage['debug_assertions'], True)
        self.assertIs(coverage['orphan_node_checks'], True)
        self.assertIs(coverage['reth_sparse_trie_coverage'], True)
        self.assertEqual(coverage['bucket_arms'], {'arena': 28674, 'arena_persistent': 28674})
        self.assertEqual(coverage['rustc_args'][:4],
                         ['--crate-name', 'reth_trie_sparse', '-C', 'debug-assertions=on'])
        self.assertEqual(coverage['rustc_cfg'],
                         ['debug_assertions', 'feature="std"', 'reth_sparse_trie_coverage'])
        self.assertIn('target-build-policy.json', provenance['sha256'])
        self.assertEqual(json.loads((run_dir / 'fuzz-session.json').read_text()),
                         {'version': 2, 'execution_bundle': 'execution'})
        # Reused artifacts without a checked target invocation cannot assert provenance.
        unchecked = temporary / 'unchecked'
        environment.update(RUN_DIR=str(unchecked), REUSE_UNCHECKED_TARGET='1')
        result = subprocess.run(['bash', str(workload / 'prepare.sh'),
                                 '--workload-config', str(config)], env=environment,
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 1)
        self.assertEqual(result.stderr.strip(),
                         'target compile policy missing; prepare with a fresh CARGO_TARGET_DIR')
        self.assertFalse((unchecked / 'execution' / 'build-info.json').exists())
        self.assertFalse((unchecked / 'fuzz-session.json').exists())


if __name__ == '__main__':
    unittest.main()
