'use strict';

var tape = require('../');
var tap = require('tap');
var concat = require('concat-stream');
var inspect = require('object-inspect');
var forEach = require('for-each');
var v = require('es-value-fixtures');

var stripFullStack = require('./common').stripFullStack;

tap.test('intercept: output', function (tt) {
	tt.plan(1);

	var test = tape.createHarness();
	var count = 0;
	test.createStream().pipe(concat(function (body) {
		tt.same(stripFullStack(body.toString('utf8')), [].concat(
			'TAP version 13',
			'# argument validation',
			v.primitives.map(function (x) {
				return 'ok ' + ++count + ' obj: ' + inspect(x) + ' is not an Object';
			}),
			v.nonPropertyKeys.map(function (x) {
				return 'ok ' + ++count + ' ' + inspect(x) + ' is not a valid property key';
			}),
			v.primitives.map(function (x) {
				return 'ok ' + ++count + ' desc: ' + inspect(x) + ' is not an Object';
			}),
			'ok ' + ++count + ' configurable false is not allowed',
			'ok ' + ++count + ' mixed data (value) and accessor (get) is not allowed',
			'ok ' + ++count + ' mixed data (writable) and accessor (set) is not allowed',
			v.nonBooleans.map(function (x) {
				return 'ok ' + ++count + ' ' + inspect(x) + ' is not a Boolean';
			}),
			'# intercepts gets/sets',
			'ok ' + ++count + ' property has expected initial value',
			'# intercepting',
			'ok ' + ++count + ' sentinel is returned from Get',
			'ok ' + ++count + ' sentinel is returned from Get again',
			'ok ' + ++count + ' sentinel is returned from Get with .call',
			'ok ' + ++count + ' undefined is returned from Get',
			'ok ' + ++count + ' undefined is returned from Get with .call',
			'ok ' + ++count + ' foo2: nonwritable property throws on Set',
			'ok ' + ++count + ' undefined is still returned from Get',
			'ok ' + ++count + ' throwing get implementation throws',
			'ok ' + ++count + ' throwing get implementation throws with .call',
			'ok ' + ++count + ' throwing set implementation throws',
			'ok ' + ++count + ' throwing set implementation throws with .call',
			'ok ' + ++count + ' fooThrowSet: get is undefined',
			'ok ' + ++count + ' getter: sentinel is returned from Get',
			'ok ' + ++count + ' getter: sentinel is returned from Get with .call',
			'ok ' + ++count + ' setter: setted value is returned from Get',
			'ok ' + ++count + ' setter: setted value is returned from Get with .call',
			'ok ' + ++count + ' sloppy: undefined is returned from Get',
			'ok ' + ++count + ' nonwritable data property in sloppy mode does not throw on Set',
			'ok ' + ++count + ' sloppy: undefined is still returned from Get',
			'ok ' + ++count + ' resultsThrowGet: results are correct',
			'ok ' + ++count + ' resultsThrowSet: results are correct',
			'ok ' + ++count + ' foo: results are correct',
			'ok ' + ++count + ' foo2: results are correct',
			'ok ' + ++count + ' sloppy: results are correct',
			'ok ' + ++count + ' getter: results are correct',
			'ok ' + ++count + ' setter: results are correct',

			'# post-intercepting',
			'ok ' + ++count + ' property is restored',
			'ok ' + ++count + ' added foo2 property is removed',
			'ok ' + ++count + ' added fooThrowGet property is removed',
			'ok ' + ++count + ' added fooThrowSet property is removed',
			'ok ' + ++count + ' added slops property is removed',
			'ok ' + ++count + ' added getter property is removed',
			'ok ' + ++count + ' added setter property is removed',
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
				function () { t.intercept(primitive, ''); },
				TypeError,
				'obj: ' + inspect(primitive) + ' is not an Object'
			);
		});

		forEach(v.nonPropertyKeys, function (nonPropertyKey) {
			t.throws(
				function () { t.intercept({}, nonPropertyKey); },
				TypeError,
				inspect(nonPropertyKey) + ' is not a valid property key'
			);
		});

		forEach(v.primitives, function (primitive) {
			t.throws(
				function () { t.intercept({}, '', primitive); },
				TypeError,
				'desc: ' + inspect(primitive) + ' is not an Object'
			);
		});
		t.throws(
			function () { t.intercept({}, '', { configurable: false }); },
			TypeError,
			'configurable false is not allowed'
		);
		t.throws(
			function () { t.intercept({}, '', { value: 1, get: function () {} }); },
			TypeError,
			'mixed data (value) and accessor (get) is not allowed'
		);
		t.throws(
			function () { t.intercept({}, '', { writable: true, set: function () {} }); },
			TypeError,
			'mixed data (writable) and accessor (set) is not allowed'
		);

		forEach(v.nonBooleans, function (nonBoolean) {
			t.throws(
				function () { t.intercept({}, '', {}, nonBoolean); },
				TypeError,
				inspect(nonBoolean) + ' is not a Boolean'
			);
		});

		t.end();
	});

	test('intercepts gets/sets', function (t) {
		var sentinel = { sentinel: true, inspect: function () { return '{ SENTINEL OBJECT }'; } };
		var o = { foo: sentinel, inspect: function () { return '{ o OBJECT }'; } };
		t.equal(o.foo, sentinel, 'property has expected initial value');

		t.test('intercepting', function (st) {
			var up = new SyntaxError('gross');

			var results = st.intercept(o, 'foo', { value: sentinel, writable: true });
			st.equal(o.foo, sentinel, 'sentinel is returned from Get');
			st.equal(o.foo, sentinel, 'sentinel is returned from Get again');
			st.equal(
				Object.getOwnPropertyDescriptor(o, 'foo').get.call(o, 1, 2, 3),
				sentinel,
				'sentinel is returned from Get with .call'
			);
			o.foo = 42;
			results.restore();

			var results2 = st.intercept(o, 'foo2');
			st.equal(o.foo2, undefined, 'undefined is returned from Get');
			st.equal(
				Object.getOwnPropertyDescriptor(o, 'foo2').get.call(o, 4, 5),
				undefined,
				'undefined is returned from Get with .call'
			);
			st.throws(
				function () { o.foo2 = 42; },
				TypeError,
				'foo2: nonwritable property throws on Set'
			);
			st.equal(o.foo2, undefined, 'undefined is still returned from Get');
			results2.restore();

			var resultsThrowGet = st.intercept(o, 'fooThrowGet', { get: function () { throw up; } });
			st.throws(
				function () { return o.fooThrowGet; },
				SyntaxError,
				'throwing get implementation throws'
			);
			st.throws(
				function () { Object.getOwnPropertyDescriptor(o, 'fooThrowGet').get.call(sentinel, 1, 2, 3); },
				SyntaxError,
				'throwing get implementation throws with .call'
			);

			o.fooThrowGet = 42;

			resultsThrowGet.restore();

			var resultsThrowSet = st.intercept(o, 'fooThrowSet', { set: function () { throw up; } });
			st.throws(
				function () { o.fooThrowSet = 42; },
				SyntaxError,
				'throwing set implementation throws'
			);
			st.throws(
				function () { Object.getOwnPropertyDescriptor(o, 'fooThrowSet').set.call(sentinel, 4, 5, 6); },
				SyntaxError,
				'throwing set implementation throws with .call'
			);
			st.equal(
				o.fooThrowSet,
				undefined,
				'fooThrowSet: get is undefined'
			);
			resultsThrowSet.restore();

			var resultsGetter = st.intercept(o, 'getter', { get: function () { return sentinel; } });
			st.equal(o.getter, sentinel, 'getter: sentinel is returned from Get');
			st.equal(Object.getOwnPropertyDescriptor(o, 'getter').get.call(sentinel, 1, 2, 3), sentinel, 'getter: sentinel is returned from Get with .call');
			resultsGetter.restore();

			var val;
			var resultsSetter = st.intercept(o, 'setter', {
				get: function () { return val; },
				set: function (x) { val = x; }
			});
			o.setter = sentinel;
			st.equal(o.setter, sentinel, 'setter: setted value is returned from Get');
			Object.getOwnPropertyDescriptor(o, 'setter').set.call(sentinel, 1, 2, 3);
			st.equal(o.setter, 1, 'setter: setted value is returned from Get with .call');
			resultsSetter.restore();

			var sloppy = t.intercept(o, 'slops', {}, false);
			st.equal(o.slops, undefined, 'sloppy: undefined is returned from Get');
			st.doesNotThrow(
				function () { o.slops = 42; },
				'nonwritable data property in sloppy mode does not throw on Set'
			);
			st.equal(o.slops, undefined, 'sloppy: undefined is still returned from Get');
			sloppy.restore();

			st.deepEqual(
				resultsThrowGet(),
				[
					{ type: 'get', success: false, threw: true, args: [], receiver: o },
					{ type: 'get', success: false, threw: true, args: [1, 2, 3], receiver: sentinel },
					{ type: 'set', value: 42, success: true, args: [42], receiver: o }
				],
				'resultsThrowGet: results are correct'
			);

			st.deepEqual(
				resultsThrowSet(),
				[
					{ type: 'set', success: false, threw: true, args: [42], receiver: o },
					{ type: 'set', success: false, threw: true, args: [4, 5, 6], receiver: sentinel },
					{ type: 'get', success: true, value: undefined, args: [], receiver: o }
				],
				'resultsThrowSet: results are correct'
			);

			st.deepEqual(
				results(),
				[
					{ type: 'get', success: true, value: sentinel, args: [], receiver: o },
					{ type: 'get', success: true, value: sentinel, args: [], receiver: o },
					{ type: 'get', success: true, value: sentinel, args: [1, 2, 3], receiver: o },
					{ type: 'set', value: 42, success: true, args: [42], receiver: o }
				],
				'foo: results are correct'
			);
			st.deepEqual(
				results2(),
				[
					{ type: 'get', success: true, value: undefined, args: [], receiver: o },
					{ type: 'get', success: true, value: undefined, args: [4, 5], receiver: o },
					{ type: 'set', success: false, value: undefined, args: [42], receiver: o },
					{ type: 'get', success: true, value: undefined, args: [], receiver: o }
				],
				'foo2: results are correct'
			);

			st.deepEqual(
				sloppy(),
				[
					{ type: 'get', success: true, value: undefined, args: [], receiver: o },
					{ type: 'set', success: false, value: undefined, args: [42], receiver: o },
					{ type: 'get', success: true, value: undefined, args: [], receiver: o }
				],
				'sloppy: results are correct'
			);

			st.deepEqual(
				resultsGetter(),
				[
					{ type: 'get', success: true, value: sentinel, args: [], receiver: o },
					{ type: 'get', success: true, value: sentinel, args: [1, 2, 3], receiver: sentinel }
				],
				'getter: results are correct'
			);

			st.deepEqual(
				resultsSetter(),
				[
					{ type: 'set', success: true, value: sentinel, args: [sentinel], receiver: o },
					{ type: 'get', success: true, value: sentinel, args: [], receiver: o },
					{ type: 'set', success: true, value: 1, args: [1, 2, 3], receiver: sentinel },
					{ type: 'get', success: true, value: 1, args: [], receiver: o }
				],
				'setter: results are correct'
			);

			st.end();
		});

		t.test('post-intercepting', function (st) {
			st.equal(o.foo, sentinel, 'property is restored');
			st.notOk('foo2' in o, 'added foo2 property is removed');
			st.notOk('fooThrowGet' in o, 'added fooThrowGet property is removed');
			st.notOk('fooThrowSet' in o, 'added fooThrowSet property is removed');
			st.notOk('slops' in o, 'added slops property is removed');
			st.notOk('getter' in o, 'added getter property is removed');
			st.notOk('setter' in o, 'added setter property is removed');

			st.end();
		});

		t.end();
	});
});
