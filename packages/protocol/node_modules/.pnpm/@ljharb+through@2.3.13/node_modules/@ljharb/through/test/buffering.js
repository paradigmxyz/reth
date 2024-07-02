'use strict';

var test = require('tape');
var through = require('../');

// must emit end before close.

test('buffering', function (assert) {
	var ts = through(function (data) {
		this.queue(data);
	}, function () {
		this.queue(null);
	});

	var ended = false;
	var actual = [];

	ts.on('data', actual.push.bind(actual));
	ts.on('end', function () {
		ended = true;
	});

	ts.write(1);
	ts.write(2);
	ts.write(3);
	assert.deepEqual(actual, [1, 2, 3]);
	ts.pause();
	ts.write(4);
	ts.write(5);
	ts.write(6);
	assert.deepEqual(actual, [1, 2, 3]);
	ts.resume();
	assert.deepEqual(actual, [1, 2, 3, 4, 5, 6]);
	ts.pause();
	ts.end();
	assert.ok(!ended);
	ts.resume();
	assert.ok(ended);
	assert.end();
});

test('buffering has data in queue, when ends', function (assert) {
	// If stream ends while paused with data in the queue, stream should still emit end after all data is written on resume.

	var ts = through(function (data) {
		this.queue(data);
	}, function () {
		this.queue(null);
	});

	var ended = false;
	var actual = [];

	ts.on('data', actual.push.bind(actual));
	ts.on('end', function () {
		ended = true;
	});

	ts.pause();
	ts.write(1);
	ts.write(2);
	ts.write(3);
	ts.end();
	assert.deepEqual(actual, [], 'no data written yet, still paused');
	assert.ok(!ended, 'end not emitted yet, still paused');
	ts.resume();
	assert.deepEqual(actual, [1, 2, 3], 'resumed, all data should be delivered');
	assert.ok(ended, 'end should be emitted once all data was delivered');
	assert.end();
});

test('data pauses', function (t) {
	var ts = through();

	var results = [];
	ts.on('data', function (data) {
		ts.pause();
		results.push(data);
	});

	t.equal(ts.paused, false, 'starts not paused');

	ts.write(1);
	t.equal(ts.paused, true, 'pauses on write');
	ts.write(2);
	t.equal(ts.paused, true, 'pauses on write');
	ts.write(3);
	t.equal(ts.paused, true, 'pauses on write');

	t.deepEqual(results, [1], 'has not been resumed');
	ts.resume();
	t.equal(ts.paused, true, 'resume unpauses, but write re-pauses');

	t.deepEqual(results, [1, 2], 'has only been resumed once');
	ts.resume();
	t.deepEqual(results, [1, 2, 3], 'has been resumed twice');

	t.end();
});
