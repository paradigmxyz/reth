'use strict';

var test = require('tape');
var through = require('../');

// must emit end before close.

test('end before close', function (t) {
	t.plan(4);

	var ts = through();
	var ended = false;
	var closed = false;

	ts.on('end', function () {
		t.ok(!closed);
		ended = true;
	});
	ts.on('close', function () {
		t.ok(ended);
		closed = true;
	});

	ts.write(1);
	ts.write(2);
	ts.write(3);
	ts.end();

	t.ok(ended);
	t.ok(closed);

	t.end();
});

test('end only once', function (t) {
	t.plan(4);

	var ts = through();
	var ended = false;

	ts.on('end', function () {
		t.equal(ended, false);
		ended = true;
	});

	ts.queue(null);
	ts.queue(null);
	ts.queue(null);

	ts.resume();

	t.equal(ended, true, 'is ended');

	ts.end();
	t.equal(ended, true, 'is still ended after end()');

	ts.end();
	t.equal(ended, true, 'is still ended after end() 2x');

	t.end();
});

test('end with value', function (t) {
	var ts = through(null, null, { autoDestroy: false });

	var results = [];
	ts.on('data', function (data) {
		results.push(data);
	});

	ts.queue(123);
	ts.queue(456);

	ts.end('end');

	t.deepEqual(results, [123, 456, 'end']);

	t.end();
});
