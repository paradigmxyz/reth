'use strict';

var tape = require('../');
var tap = require('tap');
var concat = require('concat-stream');

var stripFullStack = require('./common').stripFullStack;

function getNonFunctionMessage(fn) {
	try {
		fn();
	} catch (e) {
		return e.message;
	}
	return '';
}

var getter = function () { return 'message'; };
var messageGetterError = Object.defineProperty(
	{ custom: 'error' },
	'message',
	{ configurable: true, enumerable: true, get: getter }
);
var thrower = function () { throw messageGetterError; };

tap.test('failures', function (tt) {
	tt.plan(1);

	var test = tape.createHarness();
	test.createStream().pipe(concat(function (body) {
		tt.same(stripFullStack(body.toString('utf8')), [
			'TAP version 13',
			'# non functions',
			'not ok 1 should throw',
			'  ---',
			'    operator: throws',
			'    expected: |-',
			'      undefined',
			'    actual: |-',
			'      { [TypeError: ' + getNonFunctionMessage() + "] message: '" + getNonFunctionMessage() + "' }",
			'    at: Test.<anonymous> ($TEST/throws.js:$LINE:$COL)',
			'    stack: |-',
			String('      TypeError: ' + getNonFunctionMessage(undefined)),
			'          [... stack stripped ...]',
			'          at Test.<anonymous> ($TEST/throws.js:$LINE:$COL)',
			'          [... stack stripped ...]',
			'  ...',
			'not ok 2 should throw',
			'  ---',
			'    operator: throws',
			'    expected: |-',
			'      undefined',
			'    actual: |-',
			'      { [TypeError: ' + getNonFunctionMessage(null) + "] message: '" + getNonFunctionMessage(null) + "' }",
			'    at: Test.<anonymous> ($TEST/throws.js:$LINE:$COL)',
			'    stack: |-',
			String('      TypeError: ' + getNonFunctionMessage(null)),
			'          [... stack stripped ...]',
			'          at Test.<anonymous> ($TEST/throws.js:$LINE:$COL)',
			'          [... stack stripped ...]',
			'  ...',
			'not ok 3 should throw',
			'  ---',
			'    operator: throws',
			'    expected: |-',
			'      undefined',
			'    actual: |-',
			'      { [TypeError: ' + getNonFunctionMessage(true) + "] message: '" + getNonFunctionMessage(true) + "' }",
			'    at: Test.<anonymous> ($TEST/throws.js:$LINE:$COL)',
			'    stack: |-',
			String('      TypeError: ' + getNonFunctionMessage(true)),
			'          [... stack stripped ...]',
			'          at Test.<anonymous> ($TEST/throws.js:$LINE:$COL)',
			'          [... stack stripped ...]',
			'  ...',
			'not ok 4 should throw',
			'  ---',
			'    operator: throws',
			'    expected: |-',
			'      undefined',
			'    actual: |-',
			'      { [TypeError: ' + getNonFunctionMessage(false) + "] message: '" + getNonFunctionMessage(false) + "' }",
			'    at: Test.<anonymous> ($TEST/throws.js:$LINE:$COL)',
			'    stack: |-',
			String('      TypeError: ' + getNonFunctionMessage(false)),
			'          [... stack stripped ...]',
			'          at Test.<anonymous> ($TEST/throws.js:$LINE:$COL)',
			'          [... stack stripped ...]',
			'  ...',
			'not ok 5 should throw',
			'  ---',
			'    operator: throws',
			'    expected: |-',
			'      undefined',
			'    actual: |-',
			'      { [TypeError: ' + getNonFunctionMessage('abc') + "] message: '" + getNonFunctionMessage('abc') + "' }",
			'    at: Test.<anonymous> ($TEST/throws.js:$LINE:$COL)',
			'    stack: |-',
			String('      TypeError: ' + getNonFunctionMessage('abc')),
			'          [... stack stripped ...]',
			'          at Test.<anonymous> ($TEST/throws.js:$LINE:$COL)',
			'          [... stack stripped ...]',
			'  ...',
			'not ok 6 should throw',
			'  ---',
			'    operator: throws',
			'    expected: |-',
			'      undefined',
			'    actual: |-',
			'      { [TypeError: ' + getNonFunctionMessage(/a/g) + "] message: '" + getNonFunctionMessage(/a/g) + "' }",
			'    at: Test.<anonymous> ($TEST/throws.js:$LINE:$COL)',
			'    stack: |-',
			String('      TypeError: ' + getNonFunctionMessage(/a/g)),
			'          [... stack stripped ...]',
			'          at Test.<anonymous> ($TEST/throws.js:$LINE:$COL)',
			'          [... stack stripped ...]',
			'  ...',
			'not ok 7 should throw',
			'  ---',
			'    operator: throws',
			'    expected: |-',
			'      undefined',
			'    actual: |-',
			'      { [TypeError: ' + getNonFunctionMessage([]) + "] message: '" + getNonFunctionMessage([]) + "' }",
			'    at: Test.<anonymous> ($TEST/throws.js:$LINE:$COL)',
			'    stack: |-',
			String('      TypeError: ' + getNonFunctionMessage([])),
			'          [... stack stripped ...]',
			'          at Test.<anonymous> ($TEST/throws.js:$LINE:$COL)',
			'          [... stack stripped ...]',
			'  ...',
			'not ok 8 should throw',
			'  ---',
			'    operator: throws',
			'    expected: |-',
			'      undefined',
			'    actual: |-',
			'      { [TypeError: ' + getNonFunctionMessage({}) + "] message: '" + getNonFunctionMessage({}) + "' }",
			'    at: Test.<anonymous> ($TEST/throws.js:$LINE:$COL)',
			'    stack: |-',
			String('      TypeError: ' + getNonFunctionMessage({})),
			'          [... stack stripped ...]',
			'          at Test.<anonymous> ($TEST/throws.js:$LINE:$COL)',
			'          [... stack stripped ...]',
			'  ...',
			'# function',
			'not ok 9 should throw',
			'  ---',
			'    operator: throws',
			'    expected: undefined',
			'    actual:   undefined',
			'    at: Test.<anonymous> ($TEST/throws.js:$LINE:$COL)',
			'    stack: |-',
			'      Error: should throw',
			'          [... stack stripped ...]',
			'          at Test.<anonymous> ($TEST/throws.js:$LINE:$COL)',
			'          [... stack stripped ...]',
			'  ...',
			'# custom error messages',
			'ok 10 "message" is enumerable',
			"ok 11 { custom: 'error', message: 'message' }",
			'ok 12 getter is still the same',
			'# throws null',
			'ok 13 throws null',
			'# wrong type of error',
			'not ok 14 throws actual',
			'  ---',
			'    operator: throws',
			'    expected: |-',
			'      [Function: TypeError]',
			'    actual: |-',
			"      { [RangeError: actual!] message: 'actual!' }",
			'    at: Test.<anonymous> ($TEST/throws.js:$LINE:$COL)',
			'    stack: |-',
			'      RangeError: actual!',
			'          at Test.<anonymous> ($TEST/throws.js:$LINE:$COL)',
			'          [... stack stripped ...]',
			'  ...',
			'# non-extensible throw match',
			'ok 15 error is non-extensible',
			'ok 16 non-extensible error matches',
			'ok 17 errorWithMessage is non-extensible',
			'ok 18 non-extensible error with message matches',
			'# frozen `message` property',
			'ok 19 error is non-writable',
			'ok 20 error is non-configurable',
			'ok 21 non-writable error matches',
			'',
			'1..21',
			'# tests 21',
			'# pass  11',
			'# fail  10',
			''
		]);
	}));

	test('non functions', function (t) {
		t.plan(8);
		t['throws']();
		t['throws'](null);
		t['throws'](true);
		t['throws'](false);
		t['throws']('abc');
		t['throws'](/a/g);
		t['throws']([]);
		t['throws']({});
	});

	test('function', function (t) {
		t.plan(1);
		t['throws'](function () {});
	});

	test('custom error messages', function (t) {
		t.plan(3);
		t.equal(Object.prototype.propertyIsEnumerable.call(messageGetterError, 'message'), true, '"message" is enumerable');
		t['throws'](thrower, "{ custom: 'error', message: 'message' }");
		t.equal(Object.getOwnPropertyDescriptor(messageGetterError, 'message').get, getter, 'getter is still the same');
	});

	test('throws null', function (t) {
		t.plan(1);
		t['throws'](function () { throw null; }, 'throws null');
		t.end();
	});

	test('wrong type of error', function (t) {
		t.plan(1);
		var actual = new RangeError('actual!');
		t['throws'](function () { throw actual; }, TypeError, 'throws actual');
		t.end();
	});

	test('non-extensible throw match', { skip: !Object.seal }, function (t) {
		var error = { foo: 1 };
		Object.seal(error);
		t.throws(function () { error.x = 1; }, TypeError, 'error is non-extensible');

		t.throws(function () { throw error; }, error, 'non-extensible error matches');

		var errorWithMessage = { message: 'abc' };
		Object.seal(errorWithMessage);
		t.throws(function () { errorWithMessage.x = 1; }, TypeError, 'errorWithMessage is non-extensible');

		t.throws(function () { throw errorWithMessage; }, error, 'non-extensible error with message matches');

		t.end();
	});

	test('frozen `message` property', { skip: !Object.defineProperty }, function (t) {
		var error = { message: 'abc' };
		Object.defineProperty(error, 'message', { configurable: false, enumerable: false, writable: false });

		t.throws(function () { error.message = 'def'; }, TypeError, 'error is non-writable');
		t.throws(function () { delete error.message; }, TypeError, 'error is non-configurable');

		t.throws(function () { throw error; }, { message: 'abc' }, 'non-writable error matches');

		t.end();
	});
});
