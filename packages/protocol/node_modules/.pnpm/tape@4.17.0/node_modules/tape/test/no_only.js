'use strict';

var tap = require('tap');
var path = require('path');
var exec = require('child_process').exec;

var stripFullStack = require('./common').stripFullStack;

var tapeBin = 'node ' + path.join(__dirname, '../bin/tape');

var expectedStackTraceBug = (/^3\.[012]\.\d+$/).test(process.versions.node); // https://github.com/nodejs/node/issues/2581
var expectedExitCodeOnError = (/^0\.(?:9|10)/).test(process.versions.node) ? 8 : 1; // node v0.9 sets this exit code to 8, for some reason

tap.test(
	'Should throw error when --no-only is passed via cli and there is a .only test',
	{ todo: expectedStackTraceBug ? 'Fails on these node versions' : false },
	function (tt) {
		tt.plan(3);

		exec(tapeBin + ' --no-only "**/*.js"', {
			cwd: path.join(__dirname, 'no_only')
		}, function (err, stdout, stderr) {
			tt.same(stdout.toString('utf8'), '');
			tt.match(stripFullStack(stderr.toString('utf8')).join('\n'), /Error: `only` tests are prohibited\n/);
			tt.equal(err.code, expectedExitCodeOnError);
		});
	}
);

tap.test(
	'Should throw error when NODE_TAPE_NO_ONLY_TEST is passed via envs and there is an .only test',
	{ todo: expectedStackTraceBug ? 'Fails on these node versions' : false },
	function (tt) {
		tt.plan(3);

		exec(tapeBin + ' "**/*.js"', {
			cwd: path.join(__dirname, 'no_only'),
			env: { PATH: process.env.PATH, NODE_TAPE_NO_ONLY_TEST: 'true' }
		}, function (err, stdout, stderr) {
			tt.same(stdout.toString('utf8'), '');
			tt.match(stripFullStack(stderr.toString('utf8')).join('\n'), /Error: `only` tests are prohibited\n/);
			tt.equal(err.code, expectedExitCodeOnError);
		});
	}
);

tap.test(
	'Should override NODE_TAPE_NO_ONLY_TEST env if --no-only is passed from cli',
	{ todo: expectedStackTraceBug ? 'Fails on these node versions' : false },
	function (tt) {
		tt.plan(3);

		exec(tapeBin + ' --no-only "**/*.js"', {
			cwd: path.join(__dirname, 'no_only'),
			env: { PATH: process.env.PATH, NODE_TAPE_NO_ONLY_TEST: 'false' }
		}, function (err, stdout, stderr) {
			tt.same(stdout.toString('utf8'), '');
			tt.match(stripFullStack(stderr.toString('utf8')).join('\n'), /Error: `only` tests are prohibited\n/);
			tt.equal(err.code, expectedExitCodeOnError);
		});
	}
);

tap.test('Should run successfully if there is no only test', function (tt) {
	tt.plan(3);

	exec(tapeBin + ' --no-only "**/test-a.js"', {
		cwd: path.join(__dirname, 'no_only')
	}, function (err, stdout, stderr) {
		tt.match(stderr.toString('utf8'), /^\s*(\(node:\d+\) ExperimentalWarning: The ESM module loader is experimental\.)?\s*$/);
		tt.same(stripFullStack(stdout.toString('utf8')), [
			'TAP version 13',
			'# should pass',
			'ok 1 should be truthy',
			'',
			'1..1',
			'# tests 1',
			'# pass  1',
			'',
			'# ok',
			'',
			''
		]);
		tt.equal(err, null); // code 0
	});
});

tap.test('Should run successfully if there is an only test and no --no-only flag', function (tt) {
	tt.plan(3);

	exec(tapeBin + ' "**/test-b.js"', {
		cwd: path.join(__dirname, 'no_only')
	}, function (err, stdout, stderr) {
		tt.same(stripFullStack(stdout.toString('utf8')), [
			'TAP version 13',
			'# should pass again',
			'ok 1 should be truthy',
			'',
			'1..1',
			'# tests 1',
			'# pass  1',
			'',
			'# ok',
			'',
			''
		]);
		tt.match(stderr.toString('utf8'), /^\s*(\(node:\d+\) ExperimentalWarning: The ESM module loader is experimental\.)?\s*$/);
		tt.equal(err, null); // code 0
	});
});
