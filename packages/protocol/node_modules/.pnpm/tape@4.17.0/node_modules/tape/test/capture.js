'use strict';

var tape = require('../');
var tap = require('tap');
var concat = require('concat-stream');
var inspect = require('object-inspect');
var forEach = require('for-each');
var v = require('es-value-fixtures');

var stripFullStack = require('./common').stripFullStack;

tap.test('capture: output', function (tt) {
	tt.plan(1);

	var test = tape.createHarness();
	var count = 0;
	test.createStream().pipe(concat(function (body) {
		tt.same(stripFullStack(body.toString('utf8')), [].concat(
			'TAP version 13',
			'# argument validation',
			v.primitives.map(function (x) {
				return 'ok ' + ++count + ' ' + inspect(x) + ' is not an Object';
			}),
			v.nonPropertyKeys.map(function (x) {
				return 'ok ' + ++count + ' ' + inspect(x) + ' is not a valid property key';
			}),
			v.nonFunctions.filter(function (x) { return typeof x !== 'undefined'; }).map(function (x) {
				return 'ok ' + ++count + ' ' + inspect(x) + ' is not a function';
			}),
			'# captures calls',
			'ok ' + ++count + ' property has expected initial value',
			'# capturing',
			'ok ' + ++count + ' throwing implementation throws',
			'ok ' + ++count + ' should be equivalent',
			'ok ' + ++count + ' should be equivalent',
			'ok ' + ++count + ' should be equivalent',
			'ok ' + ++count + ' should be equivalent',
			'ok ' + ++count + ' should be equivalent',
			'# post-capturing',
			'ok ' + ++count + ' property is restored',
			'ok ' + ++count + ' added property is removed',
			'',
			'1..' + count,
			'# tests ' + count,
			'# pass  ' + count,
			'',
			'# ok',
			''
		));
	}));

	test('argument validation', function (t) {
		forEach(v.primitives, function (primitive) {
			t.throws(
				function () { t.capture(primitive, ''); },
				TypeError,
				inspect(primitive) + ' is not an Object'
			);
		});

		forEach(v.nonPropertyKeys, function (nonPropertyKey) {
			t.throws(
				function () { t.capture({}, nonPropertyKey); },
				TypeError,
				inspect(nonPropertyKey) + ' is not a valid property key'
			);
		});

		forEach(v.nonFunctions, function (nonFunction) {
			if (typeof nonFunction !== 'undefined') {
				t.throws(
					function () { t.capture({}, '', nonFunction); },
					TypeError,
					inspect(nonFunction) + ' is not a function'
				);
			}
		});

		t.end();
	});

	test('captures calls', function (t) {
		var sentinel = { sentinel: true, inspect: function () { return '{ SENTINEL OBJECT }'; } };
		var o = { foo: sentinel, inspect: function () { return '{ o OBJECT }'; } };
		t.equal(o.foo, sentinel, 'property has expected initial value');

		t.test('capturing', function (st) {
			var results = st.capture(o, 'foo', function () { return sentinel; });
			var results2 = st.capture(o, 'foo2');
			var up = new SyntaxError('foo');
			var resultsThrow = st.capture(o, 'fooThrow', function () { throw up; });

			o.foo(1, 2, 3);
			o.foo(3, 4, 5);
			o.foo2.call(sentinel, 1);
			st.throws(
				function () { o.fooThrow(1, 2, 3); },
				SyntaxError,
				'throwing implementation throws'
			);

			st.deepEqual(results(), [
				{ args: [1, 2, 3], receiver: o, returned: sentinel },
				{ args: [3, 4, 5], receiver: o, returned: sentinel }
			]);
			st.deepEqual(results(), []);

			o.foo(6, 7, 8);
			st.deepEqual(results(), [
				{ args: [6, 7, 8], receiver: o, returned: sentinel }
			]);

			st.deepEqual(results2(), [
				{ args: [1], receiver: sentinel, returned: undefined }
			]);
			st.deepEqual(resultsThrow(), [
				{ args: [1, 2, 3], receiver: o, threw: true }
			]);

			st.end();
		});

		t.test('post-capturing', function (st) {
			st.equal(o.foo, sentinel, 'property is restored');
			st.notOk('foo2' in o, 'added property is removed');

			st.end();
		});

		t.end();
	});
});
