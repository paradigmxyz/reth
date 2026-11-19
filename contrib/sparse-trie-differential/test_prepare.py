#!/usr/bin/env python3
"""Check the exact compiler wrapper emitted by preparation without compiling Rust."""
import hashlib
import os
import pathlib
import runpy
import sys
import tempfile
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


if __name__ == '__main__':
    unittest.main()
