'use strict';

var test = require('tape');
var hasPropertyDescriptors = require('has-property-descriptors');
var v = require('es-value-fixtures');
var forEach = require('for-each');
var inspect = require('object-inspect');

var mockProperty = require('../');

var sentinel = { sentinel: true };
var getter = function () {};
var setter = function (value) {}; // eslint-disable-line no-unused-vars

test('mockProperty', function (t) {
	t.equal(typeof mockProperty, 'function', 'is a function');

	t.test('errors', function (st) {
		var o = {};
		var p = 'property';

		forEach(v.nonBooleans, function (nonBoolean) {
			st['throws'](
				function () { mockProperty(o, p, { nonEnumerable: nonBoolean }); },
				TypeError,
				'nonEnumerable: ' + inspect(nonBoolean) + ' is not a boolean'
			);
			st['throws'](
				function () { mockProperty(o, p, { nonWritable: nonBoolean }); },
				TypeError,
				'nonWritable: ' + inspect(nonBoolean) + ' is not a boolean'
			);
			st['throws'](
				function () { mockProperty(o, p, { 'delete': nonBoolean }); },
				TypeError,
				'delete: ' + inspect(nonBoolean) + ' is not a boolean'
			);
		});

		st['throws'](
			function () { mockProperty(o, p, { get: function () {}, value: undefined }); },
			TypeError,
			'get and value are mutually exclusive'
		);
		st['throws'](
			function () { mockProperty(o, p, { set: function () {}, value: undefined }); },
			TypeError,
			'set and value are mutually exclusive'
		);
		st['throws'](
			function () { mockProperty(o, p, { get: function () {}, nonWritable: true }); },
			TypeError,
			'get and nonWritable are mutually exclusive'
		);
		st['throws'](
			function () { mockProperty(o, p, { set: function () {}, nonWritable: true }); },
			TypeError,
			'set and nonWritable are mutually exclusive'
		);

		forEach(v.nonFunctions, function (nonFunction) {
			st['throws'](
				function () { mockProperty(o, p, { get: nonFunction }); },
				TypeError,
				'get: ' + inspect(nonFunction) + ' is not a function'
			);
			st['throws'](
				function () { mockProperty(o, p, { set: nonFunction }); },
				TypeError,
				'set: ' + inspect(nonFunction) + ' is not a function'
			);
		});

		st['throws'](
			function () { mockProperty(o, p, { 'delete': true, get: function () {} }); },
			TypeError,
			'delete and get are mutually exclusive'
		);
		st['throws'](
			function () { mockProperty(o, p, { 'delete': true, set: function () {} }); },
			TypeError,
			'delete and set are mutually exclusive'
		);
		st['throws'](
			function () { mockProperty(o, p, { 'delete': true, value: undefined }); },
			TypeError,
			'delete and value are mutually exclusive'
		);
		st['throws'](
			function () { mockProperty(o, p, { 'delete': true, nonWritable: false }); },
			TypeError,
			'delete and nonWritable are mutually exclusive'
		);
		st['throws'](
			function () { mockProperty(o, p, { 'delete': true, nonEnumerable: false }); },
			TypeError,
			'delete and nonEnumerable are mutually exclusive'
		);

		st.end();
	});

	t.test('data -> data', function (st) {
		var obj = { a: sentinel };

		t.comment('mockProperty(…)');
		var restore = mockProperty(obj, 'a', { value: obj });

		st.ok('a' in obj, 'property still exists');
		st.equal(obj.a, obj, 'obj has expected value');

		t.comment('restore');
		restore();

		st.ok('a' in obj, 'property is restored');
		st.equal(obj.a, sentinel, 'data property holds sentinel');

		st.end();
	});

	t.test('data: enumerable -> nonEnumerable', { skip: !hasPropertyDescriptors() }, function (st) {
		var obj = { a: sentinel };
		st.ok(Object.prototype.propertyIsEnumerable.call(obj, 'a'), 'starts enumerable');

		t.comment('mockProperty(…)');
		var restore = mockProperty(obj, 'a', { nonEnumerable: true });

		st.ok('a' in obj, 'property still exists');
		st.equal(obj.a, sentinel, 'data property still holds sentinel');
		st.notOk(Object.prototype.propertyIsEnumerable.call(obj, 'a'), 'is not enumerable');

		t.comment('restore');
		restore();

		st.ok('a' in obj, 'property is restored');
		st.equal(obj.a, sentinel, 'data property again holds sentinel');
		st.ok(Object.prototype.propertyIsEnumerable.call(obj, 'a'), 'ends enumerable');

		st.end();
	});

	t.test('data -> absent', function (st) {
		var obj = { a: sentinel };

		t.comment('mockProperty(…)');
		var restore = mockProperty(obj, 'a', { 'delete': true });

		st.notOk('a' in obj, 'property is deleted');

		t.comment('restore');
		restore();

		st.ok('a' in obj, 'property is restored');
		st.equal(obj.a, sentinel, 'object value is restored to sentinel');

		st.end();
	});

	t.test('absent -> data', function (st) {
		var obj = {};

		st.notOk('a' in obj, 'property is initially absent');

		t.comment('mockProperty(…)');
		var restore = mockProperty(obj, 'a', { value: sentinel });

		st.ok('a' in obj, 'property exists');
		st.equal(obj.a, sentinel, 'data property holds sentinel');

		t.comment('restore');
		restore();

		st.notOk('a' in obj, 'property is absent again');

		st.end();
	});

	t.test('absent -> absent', function (st) {
		var obj = {};

		st.notOk('a' in obj, 'property is initially absent');

		t.comment('mockProperty(…)');
		var restore = mockProperty(obj, 'a', { 'delete': true });

		st.notOk('a' in obj, 'property still does not exist');

		t.comment('restore');
		restore();

		st.notOk('a' in obj, 'property remains absent');

		st.end();
	});

	t.test('getter', { skip: !hasPropertyDescriptors() }, function (st) {
		st.test('data: nonconfigurable, nonwritable, change value', function (s2t) {
			var o = {};
			var p = 'property';
			o[p] = sentinel;
			Object.defineProperty(o, p, { configurable: false, writable: false });
			s2t.deepEqual(
				Object.getOwnPropertyDescriptor(o, p),
				{
					configurable: false,
					enumerable: true,
					value: sentinel,
					writable: false
				},
				'precondition: expected descriptor'
			);

			s2t['throws'](
				function () { mockProperty(o, p, { value: 42 }); },
				TypeError,
				'nonconfigurable property throws'
			);

			s2t.end();
		});

		st.test('data: nonconfigurable, nonwritable, same value', function (s2t) {
			var o = {};
			var p = 'property';
			o[p] = sentinel;
			Object.defineProperty(o, p, { configurable: false, writable: false });
			s2t.deepEqual(
				Object.getOwnPropertyDescriptor(o, p),
				{
					configurable: false,
					enumerable: true,
					value: sentinel,
					writable: false
				},
				'precondition: expected descriptor'
			);

			s2t.doesNotThrow(
				function () { mockProperty(o, p, { value: sentinel }); },
				'same value is a noop'
			);

			s2t.end();
		});

		st.test('data: nonconfigurable, writable, change value', function (s2t) {
			var o = {};
			var p = 'property';
			o[p] = sentinel;
			Object.defineProperty(o, p, { configurable: false, writable: true });
			s2t.deepEqual(
				Object.getOwnPropertyDescriptor(o, p),
				{
					configurable: false,
					enumerable: true,
					value: sentinel,
					writable: true
				},
				'precondition: expected descriptor'
			);

			s2t.doesNotThrow(
				function () { mockProperty(o, p, { value: { wrong: true } })(); },
				'writable property can change value'
			);
			s2t.deepEqual(
				Object.getOwnPropertyDescriptor(o, p),
				{
					configurable: false,
					enumerable: true,
					value: sentinel,
					writable: true
				},
				'postcondition: expected descriptor'
			);

			s2t.end();
		});

		st.test('data: nonconfigurable, writable, change writability', function (s2t) {
			var o = {};
			var p = 'property';
			o[p] = sentinel;
			Object.defineProperty(o, p, { configurable: false, writable: true });
			s2t.deepEqual(
				Object.getOwnPropertyDescriptor(o, p),
				{
					configurable: false,
					enumerable: true,
					value: sentinel,
					writable: true
				},
				'precondition: expected descriptor'
			);

			s2t['throws'](
				function () { mockProperty(o, p, { nonWritable: true }); },
				TypeError,
				'nonconfigurable property throws'
			);

			s2t.end();
		});

		st.test('data: nonconfigurable, nonwritable, change writability', function (s2t) {
			var o = {};
			var p = 'property';
			o[p] = sentinel;
			Object.defineProperty(o, p, { configurable: false, writable: false });
			s2t.deepEqual(
				Object.getOwnPropertyDescriptor(o, p),
				{
					configurable: false,
					enumerable: true,
					value: sentinel,
					writable: false
				},
				'precondition: expected descriptor'
			);

			s2t['throws'](
				function () { mockProperty(o, p, { nonWritable: false }); },
				TypeError,
				'nonconfigurable property throws'
			);

			s2t.end();
		});

		st.test('data: nonconfigurable, writable, nonenumerable', function (s2t) {
			var o = {};
			var p = 'property';
			o[p] = sentinel;
			Object.defineProperty(o, p, { configurable: false, enumerable: false, writable: true });
			s2t.deepEqual(
				Object.getOwnPropertyDescriptor(o, p),
				{
					configurable: false,
					enumerable: false,
					value: sentinel,
					writable: true
				},
				'precondition: expected descriptor'
			);

			s2t.doesNotThrow(
				function () { mockProperty(o, p, { nonEnumerable: true })(); },
				'nonconfigurable nonenumerable, to nonenumerable, is a noop'
			);

			s2t['throws'](
				function () { mockProperty(o, p, { nonEnumerable: false })(); },
				TypeError,
				'nonconfigurable nonenumerable, to enumerable, throws'
			);

			s2t.end();
		});

		st.test('data: nonconfigurable, writable, enumerable', function (s2t) {
			var o = {};
			var p = 'property';
			o[p] = sentinel;
			Object.defineProperty(o, p, { configurable: false, enumerable: true, writable: true });
			s2t.deepEqual(
				Object.getOwnPropertyDescriptor(o, p),
				{
					configurable: false,
					enumerable: true,
					value: sentinel,
					writable: true
				},
				'precondition: expected descriptor'
			);

			s2t.doesNotThrow(
				function () { mockProperty(o, p, { nonEnumerable: false })(); },
				'nonconfigurable enumerable, to enumerable, is a noop'
			);

			s2t['throws'](
				function () { mockProperty(o, p, { nonEnumerable: true }); },
				TypeError,
				'nonconfigurable enumerable, to nonenumerable, throws'
			);

			s2t.end();
		});

		st.test('nonconfigurable data -> accessor', function (s2t) {
			var o = {};
			var p = 'property';
			o[p] = sentinel;
			Object.defineProperty(o, p, { configurable: false });
			s2t.deepEqual(
				Object.getOwnPropertyDescriptor(o, p),
				{
					configurable: false,
					enumerable: true,
					value: sentinel,
					writable: true
				},
				'precondition: expected descriptor'
			);

			s2t['throws'](
				function () { mockProperty(o, p, { get: getter }); },
				TypeError,
				'nonconfigurable data, with getter, throws'
			);

			s2t['throws'](
				function () { mockProperty(o, p, { set: setter }); },
				TypeError,
				'nonconfigurable data, with getter, throws'
			);

			s2t['throws'](
				function () { mockProperty(o, p, { get: getter, set: setter }); },
				TypeError,
				'nonconfigurable data, with both, throws'
			);

			s2t.end();
		});

		st.test('nonconfigurable accessor -> data', function (s2t) {
			var o = {};
			var p = 'property';
			o[p] = sentinel;
			Object.defineProperty(o, p, { configurable: false, get: getter, set: setter });
			s2t.deepEqual(
				Object.getOwnPropertyDescriptor(o, p),
				{
					configurable: false,
					enumerable: true,
					get: getter,
					set: setter
				},
				'precondition: expected descriptor'
			);

			s2t['throws'](
				function () { mockProperty(o, p, { value: sentinel }); },
				TypeError,
				'nonconfigurable both, with data, throws'
			);

			s2t.end();
		});

		st.test('accessor: nonconfigurable', function (s2t) {
			var o = {};
			var p = 'property';
			o[p] = sentinel;
			Object.defineProperty(o, p, { configurable: false, get: getter, set: setter });
			s2t.deepEqual(
				Object.getOwnPropertyDescriptor(o, p),
				{
					configurable: false,
					enumerable: true,
					get: getter,
					set: setter
				},
				'precondition: expected descriptor'
			);

			s2t.doesNotThrow(
				function () { mockProperty(o, p, { get: getter })(); },
				'same getter is a noop'
			);

			s2t.doesNotThrow(
				function () { mockProperty(o, p, { set: setter })(); },
				'same setter is a noop'
			);

			s2t['throws'](
				function () { mockProperty(o, p, { get: function () {} }); },
				TypeError,
				'nonconfigurable both, changing getter, throws'
			);

			s2t['throws'](
				function () { mockProperty(o, p, { set: function (value) {} }); }, // eslint-disable-line no-unused-vars
				TypeError,
				'nonconfigurable both, changing setter, throws'
			);

			s2t['throws'](
				function () { mockProperty(o, p, { enumerable: false }); },
				TypeError,
				'nonconfigurable both, changing setter, throws'
			);

			s2t.end();
		});

		st.test('getter -> getter', function (s2t) {
			var calls = 0;
			var obj = { a: 1 };
			Object.defineProperty(obj, 'a', {
				get: function () {
					calls += 1;
					return 'calls: ' + calls;
				}
			});

			s2t.ok('a' in obj, 'property exists');
			s2t.equal(obj.a, 'calls: 1', 'getter returns 1 call');

			t.comment('mockProperty(…)');
			var restore = mockProperty(obj, 'a', {
				get: function () {
					calls += 100;
					return 'calls: ' + calls;
				}
			});

			s2t.ok('a' in obj, 'property still exists');
			s2t.equal(obj.a, 'calls: 101', 'getter returns 101 calls');

			t.comment('restore');
			restore();

			s2t.ok('a' in obj, 'property still exists');
			s2t.equal(obj.a, 'calls: 102', 'getter returns 102 calls');

			s2t.end();
		});

		st.test('getter -> setter', function (s2t) {
			var calls = 0;
			var obj = { a: 1 };
			Object.defineProperty(obj, 'a', {
				get: function () {
					calls += 1;
					return 'calls: ' + calls;
				}
			});

			s2t.ok('a' in obj, 'property exists');
			s2t.equal(obj.a, 'calls: 1', 'getter returns 1 call');

			var holder;
			t.comment('mockProperty(…)');
			var restore = mockProperty(obj, 'a', {
				get: undefined,
				set: function (value) {
					holder = 'holder mocked: ' + value;
				}
			});

			s2t.ok('a' in obj, 'property still exists');
			obj.a = 'second';
			s2t.equal(holder, 'holder mocked: second', 'setter was invoked ("second")');
			s2t.equal(obj.a, undefined, 'getter returns undefined');

			t.comment('restore');
			restore();

			s2t.ok('a' in obj, 'property still exists');
			s2t.equal(obj.a, 'calls: 2', 'getter returns 2 calls');

			s2t.end();
		});

		st.test('getter -> both', function (s2t) {
			var calls = 0;
			var obj = { a: 1 };
			Object.defineProperty(obj, 'a', {
				get: function () {
					calls += 1;
					return 'calls: ' + calls;
				}
			});

			s2t.ok('a' in obj, 'property exists');
			s2t.equal(obj.a, 'calls: 1', 'getter returns 1 call');

			var holder;

			t.comment('mockProperty(…)');
			var restore = mockProperty(obj, 'a', {
				set: function (value) {
					holder = 'holder mocked: ' + value;
				}
			});

			s2t.ok('a' in obj, 'property still exists');
			obj.a = 'second';
			s2t.equal(holder, 'holder mocked: second', 'setter was invoked ("second")');
			s2t.equal(obj.a, 'calls: 2', 'getter returns 2 calls');

			t.comment('restore');
			restore();

			s2t.ok('a' in obj, 'property still exists');
			s2t.equal(holder, 'holder mocked: second', 'setter was not invoked ("third")');
			s2t.equal(obj.a, 'calls: 3', 'getter returns 3 calls');

			s2t.end();
		});

		st.test('getter -> data', function (s2t) {
			var calls = 0;
			var obj = { a: 1 };
			Object.defineProperty(obj, 'a', {
				get: function () {
					calls += 1;
					return 'calls: ' + calls;
				}
			});

			s2t.ok('a' in obj, 'property exists');
			s2t.equal(obj.a, 'calls: 1', 'getter returns 1 call');

			t.comment('mockProperty(…)');
			var restore = mockProperty(obj, 'a', {
				value: sentinel
			});

			s2t.ok('a' in obj, 'property still exists');
			s2t.equal(obj.a, sentinel, 'data property holds sentinel');

			t.comment('restore');
			restore();

			s2t.ok('a' in obj, 'property still exists');
			s2t.equal(obj.a, 'calls: 2', 'getter returns 2 calls');

			s2t.end();
		});

		st.test('getter -> absent', function (s2t) {
			var calls = 0;
			var obj = { a: 1 };
			Object.defineProperty(obj, 'a', {
				get: function () {
					calls += 1;
					return 'calls: ' + calls;
				}
			});

			s2t.ok('a' in obj, 'property exists');
			s2t.equal(obj.a, 'calls: 1', 'getter returns 1 call');

			var restore = mockProperty(obj, 'a', {
				'delete': true
			});

			s2t.notOk('a' in obj, 'property is deleted');

			t.comment('restore');
			restore();

			s2t.ok('a' in obj, 'property still exists');
			s2t.equal(obj.a, 'calls: 2', 'getter returns 2 calls');

			s2t.end();
		});

		st.test('setter -> getter', function (s2t) {
			var calls = 0;
			var holder;
			var obj = { a: 1 };
			Object.defineProperty(obj, 'a', {
				set: function (value) {
					holder = 'holder: ' + value;
				}
			});

			s2t.ok('a' in obj, 'property exists');
			obj.a = 'first';
			s2t.equal(holder, 'holder: first', 'setter was invoked ("first")');
			s2t.equal(obj.a, undefined, 'getter returns undefined');

			t.comment('mockProperty(…)');
			var restore = mockProperty(obj, 'a', {
				get: function () {
					calls += 1;
					return 'calls: ' + calls;
				},
				set: undefined
			});

			s2t.ok('a' in obj, 'property still exists');
			s2t.equal(obj.a, 'calls: 1', 'getter returns 1 calls');
			s2t['throws'](
				function () { obj.a = 42; },
				TypeError,
				'no setter, throws'
			);

			t.comment('restore');
			restore();

			s2t.ok('a' in obj, 'property still exists');
			obj.a = 'third';
			s2t.equal(holder, 'holder: third', 'setter was invoked ("third")');
			s2t.equal(obj.a, undefined, 'getter returns undefined');

			s2t.end();
		});

		st.test('setter -> both', function (s2t) {
			var calls = 0;
			var holder;
			var obj = { a: 1 };
			Object.defineProperty(obj, 'a', {
				set: function (value) {
					holder = 'holder: ' + value;
				}
			});

			s2t.ok('a' in obj, 'property exists');
			obj.a = 'first';
			s2t.equal(holder, 'holder: first', 'setter was invoked ("first")');
			s2t.equal(obj.a, undefined, 'no getter, undefined');

			t.comment('mockProperty(…)');
			var restore = mockProperty(obj, 'a', {
				get: function () {
					calls += 1;
					return 'calls: ' + calls;
				}
			});

			s2t.ok('a' in obj, 'property still exists');
			obj.a = 'second';
			s2t.equal(holder, 'holder: second', 'setter was invoked ("second")');
			s2t.equal(obj.a, 'calls: 1', 'getter returns 1 calls');

			t.comment('restore');
			restore();

			s2t.ok('a' in obj, 'property still exists');
			obj.a = 'third';
			s2t.equal(holder, 'holder: third', 'setter was invoked ("third")');
			s2t.equal(obj.a, undefined, 'no getter, undefined');

			s2t.end();
		});

		st.test('setter -> setter', function (s2t) {
			var obj = { a: 1 };
			var holder;
			Object.defineProperty(obj, 'a', {
				set: function (value) {
					holder = 'holder: ' + value;
				}
			});

			s2t.ok('a' in obj, 'property exists');
			obj.a = 'first';
			s2t.equal(holder, 'holder: first', 'setter was invoked ("first")');

			t.comment('mockProperty(…)');
			var restore = mockProperty(obj, 'a', {
				set: function (value) {
					holder = 'holder mocked: ' + value;
				}
			});

			s2t.ok('a' in obj, 'property exists');
			obj.a = 'second';
			s2t.equal(holder, 'holder mocked: second', 'setter was invoked ("second")');

			t.comment('restore');
			restore();

			s2t.ok('a' in obj, 'property exists');
			obj.a = 'third';
			s2t.equal(holder, 'holder: third', 'setter was invoked ("third")');

			s2t.end();
		});

		st.test('setter -> data', function (s2t) {
			var obj = { a: 1 };
			var holder;
			Object.defineProperty(obj, 'a', {
				set: function (value) {
					holder = 'holder: ' + value;
				}
			});

			s2t.ok('a' in obj, 'property exists');
			s2t.equal(obj.a, undefined, 'no getter, undefined');
			obj.a = 'first';
			s2t.equal(holder, 'holder: first', 'setter was invoked ("first")');

			t.comment('mockProperty(…)');
			var restore = mockProperty(obj, 'a', {
				value: sentinel
			});

			s2t.ok('a' in obj, 'property exists');
			s2t.equal(obj.a, sentinel, 'data property holds sentinel');
			obj.a = 'second';
			s2t.equal(holder, 'holder: first', 'setter was not invoked ("second")');
			s2t.notEqual(obj.a, sentinel, 'data property no longer holds sentinel');

			t.comment('restore');
			restore();

			s2t.ok('a' in obj, 'property exists');
			s2t.equal(obj.a, undefined, 'no getter, undefined');
			obj.a = 'third';
			s2t.equal(holder, 'holder: third', 'setter was invoked ("third")');

			s2t.end();
		});

		st.test('setter -> absent', function (s2t) {
			var obj = { a: 1 };
			var holder;
			Object.defineProperty(obj, 'a', {
				set: function (value) {
					holder = 'holder: ' + value;
				}
			});

			s2t.ok('a' in obj, 'property exists');
			obj.a = 'first';
			s2t.equal(holder, 'holder: first', 'setter was invoked ("first")');

			t.comment('mockProperty(…)');
			var restore = mockProperty(obj, 'a', {
				'delete': true
			});

			s2t.notOk('a' in obj, 'property no longer exists');

			t.comment('restore');
			restore();

			s2t.ok('a' in obj, 'property exists');
			obj.a = 'third';
			s2t.equal(holder, 'holder: third', 'setter was invoked ("third")');

			s2t.end();
		});

		st.test('data -> getter', function (s2t) {
			var calls = 0;
			var obj = { a: sentinel };

			s2t.ok('a' in obj, 'property exists');
			s2t.equal(obj.a, sentinel, 'data property holds sentinel');

			t.comment('mockProperty(…)');
			var restore = mockProperty(obj, 'a', {
				get: function () {
					calls += 100;
					return 'calls: ' + calls;
				}
			});

			s2t.ok('a' in obj, 'property still exists');
			s2t.equal(obj.a, 'calls: 100', 'getter returns 100 calls');
			s2t['throws'](
				function () { obj.a = 42; },
				TypeError,
				'no setter, throws'
			);

			t.comment('restore');
			restore();

			s2t.ok('a' in obj, 'property still exists');
			s2t.equal(obj.a, sentinel, 'data property holds sentinel');

			s2t.end();
		});

		st.test('data -> setter', function (s2t) {
			var obj = { a: sentinel };

			s2t.ok('a' in obj, 'property exists');
			s2t.equal(obj.a, sentinel, 'data property holds sentinel');

			var holder;
			t.comment('mockProperty(…)');
			var restore = mockProperty(obj, 'a', {
				set: function (value) {
					holder = 'holder mocked: ' + value;
				}
			});

			s2t.ok('a' in obj, 'property still exists');
			obj.a = 'second';
			s2t.equal(holder, 'holder mocked: second', 'setter was invoked ("second")');
			s2t.equal(obj.a, undefined, 'no getter, undefined');

			t.comment('restore');
			restore();

			s2t.ok('a' in obj, 'property still exists');
			s2t.equal(obj.a, sentinel, 'data property holds sentinel');
			s2t.equal(holder, 'holder mocked: second', 'setter was not invoked ("second")');

			s2t.end();
		});

		st.test('data -> both', function (s2t) {
			var calls = 0;
			var obj = { a: sentinel };

			s2t.ok('a' in obj, 'property exists');
			s2t.equal(obj.a, sentinel, 'data property holds sentinel');

			var holder;
			t.comment('mockProperty(…)');
			var restore = mockProperty(obj, 'a', {
				get: function () {
					calls += 100;
					return 'calls: ' + calls;
				},
				set: function (value) {
					holder = 'holder mocked: ' + value;
				}
			});

			s2t.ok('a' in obj, 'property still exists');
			obj.a = 'second';
			s2t.equal(holder, 'holder mocked: second', 'setter was invoked ("second")');
			s2t.equal(obj.a, 'calls: 100', 'getter returns 100 calls');

			t.comment('restore');
			restore();

			s2t.ok('a' in obj, 'property still exists');
			s2t.equal(obj.a, sentinel, 'data property holds sentinel');
			s2t.equal(holder, 'holder mocked: second', 'setter was not invoked ("second")');

			s2t.end();
		});

		st.test('absent -> getter', function (s2t) {
			var calls = 0;
			var obj = {};

			s2t.notOk('a' in obj, 'property does not exist');

			t.comment('mockProperty(…)');
			var restore = mockProperty(obj, 'a', {
				get: function () {
					calls += 100;
					return 'calls: ' + calls;
				}
			});

			s2t.ok('a' in obj, 'property exists');
			s2t.equal(obj.a, 'calls: 100', 'getter returns 100 calls');

			t.comment('restore');
			restore();

			s2t.notOk('a' in obj, 'property no longer exists');

			s2t.end();
		});

		st.test('absent -> setter', function (s2t) {
			var obj = {};

			s2t.notOk('a' in obj, 'property does not exist');

			var holder;
			t.comment('mockProperty(…)');
			var restore = mockProperty(obj, 'a', {
				set: function (value) {
					holder = 'holder mocked: ' + value;
				}
			});

			s2t.ok('a' in obj, 'property still exists');
			obj.a = 'second';
			s2t.equal(holder, 'holder mocked: second', 'setter was invoked ("second")');
			s2t.equal(obj.a, undefined, 'no getter, undefined');

			t.comment('restore');
			restore();

			s2t.notOk('a' in obj, 'property no longer exists');

			s2t.end();
		});

		st.test('absent -> both', function (s2t) {
			var calls = 0;
			var obj = {};

			s2t.notOk('a' in obj, 'property does not exist');

			var holder;
			t.comment('mockProperty(…)');
			var restore = mockProperty(obj, 'a', {
				get: function () {
					calls += 100;
					return 'calls: ' + calls;
				},
				set: function (value) {
					holder = 'holder mocked: ' + value;
				}
			});

			s2t.ok('a' in obj, 'property still exists');
			obj.a = 'second';
			s2t.equal(holder, 'holder mocked: second', 'setter was invoked ("second")');
			s2t.equal(obj.a, 'calls: 100', 'getter returns 100 calls');

			t.comment('restore');
			restore();

			s2t.notOk('a' in obj, 'property no longer exists');

			s2t.end();
		});

		st.test('both -> absent', function (s2t) {
			var calls = 0;
			var holder;
			var obj = { a: 1 };
			Object.defineProperty(obj, 'a', {
				get: function () {
					calls += 1;
					return 'calls: ' + calls;
				},
				set: function (value) {
					holder = 'holder: ' + value;
				}
			});

			s2t.ok('a' in obj, 'property exists');
			s2t.equal(obj.a, 'calls: 1', 'getter returns 1 calls');
			obj.a = 'first';
			s2t.equal(holder, 'holder: first', 'setter was invoked ("first")');
			s2t.equal(obj.a, 'calls: 2', 'getter returns 2 calls');

			var restore = mockProperty(obj, 'a', {
				'delete': true
			});

			s2t.notOk('a' in obj, 'property is deleted');

			t.comment('restore');
			restore();

			s2t.ok('a' in obj, 'property still exists');
			s2t.equal(obj.a, 'calls: 3', 'getter returns 3 calls');
			obj.a = 'third';
			s2t.equal(holder, 'holder: third', 'setter was invoked ("third")');
			s2t.equal(obj.a, 'calls: 4', 'getter returns 4 calls');

			s2t.end();
		});

		st.test('both -> getter', function (s2t) {
			var calls = 0;
			var holder;
			var obj = { a: 1 };
			Object.defineProperty(obj, 'a', {
				get: function () {
					calls += 1;
					return 'calls: ' + calls;
				},
				set: function (value) {
					holder = 'holder: ' + value;
				}
			});

			s2t.ok('a' in obj, 'property exists');
			s2t.equal(obj.a, 'calls: 1', 'getter returns 1 calls');
			obj.a = 'first';
			s2t.equal(holder, 'holder: first', 'setter was invoked ("first")');
			s2t.equal(obj.a, 'calls: 2', 'getter returns 2 calls');

			var restore = mockProperty(obj, 'a', {
				set: undefined
			});

			s2t.ok('a' in obj, 'property still exists');
			s2t.equal(obj.a, 'calls: 3', 'getter returns 3 calls');
			s2t['throws'](
				function () { obj.a = 'second'; },
				TypeError,
				'no setter, throws'
			);
			s2t.equal(obj.a, 'calls: 4', 'getter returns 4 calls');

			t.comment('restore');
			restore();

			s2t.ok('a' in obj, 'property still exists');
			s2t.equal(obj.a, 'calls: 5', 'getter returns 5 calls');
			obj.a = 'third';
			s2t.equal(holder, 'holder: third', 'setter was invoked ("third")');
			s2t.equal(obj.a, 'calls: 6', 'getter returns 6 calls');

			s2t.end();
		});

		st.test('both -> setter', function (s2t) {
			var calls = 0;
			var holder;
			var obj = { a: 1 };
			Object.defineProperty(obj, 'a', {
				get: function () {
					calls += 1;
					return 'calls: ' + calls;
				},
				set: function (value) {
					holder = 'holder: ' + value;
				}
			});

			s2t.ok('a' in obj, 'property exists');
			s2t.equal(obj.a, 'calls: 1', 'getter returns 1 calls');
			obj.a = 'first';
			s2t.equal(holder, 'holder: first', 'setter was invoked ("first")');
			s2t.equal(obj.a, 'calls: 2', 'getter returns 2 calls');

			var restore = mockProperty(obj, 'a', {
				get: undefined
			});

			s2t.ok('a' in obj, 'property still exists');
			s2t.equal(obj.a, undefined, 'no getter, undefined');
			obj.a = 'second';
			s2t.equal(holder, 'holder: second', 'setter was invoked ("second")');
			s2t.equal(obj.a, undefined, 'no getter, undefined');

			t.comment('restore');
			restore();

			s2t.ok('a' in obj, 'property still exists');
			s2t.equal(obj.a, 'calls: 3', 'getter returns 3 calls');
			obj.a = 'third';
			s2t.equal(holder, 'holder: third', 'setter was invoked ("third")');
			s2t.equal(obj.a, 'calls: 4', 'getter returns 4 calls');

			s2t.end();
		});

		st.end();
	});

	t.test('array length bug', function (st) {
		var a = [1, , 3]; // eslint-disable-line no-sparse-arrays
		st.equal(a.length, 3, 'length starts at 3');

		mockProperty(a, 'length', { value: 5 });

		st.equal(a.length, 5, 'length ends at 5');

		st.end();
	});

	t.end();
});
