'use strict';

var through = require('@ljharb/through');

var nextTick = typeof setImmediate === 'undefined'
	? process.nextTick
	: setImmediate;

module.exports = function (write, end) {
	var tr = through(write, end);
	tr.pause();
	var resume = tr.resume;
	var pause = tr.pause;
	var paused = false;

	tr.pause = function () {
		paused = true;
		return pause.apply(this, arguments);
	};

	tr.resume = function () {
		paused = false;
		return resume.apply(this, arguments);
	};

	nextTick(function () {
		if (!paused) { tr.resume(); }
	});

	return tr;
};
