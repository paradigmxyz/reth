'use strict';

var Stream = require('stream').Stream;
var callBind = require('call-bind');

/** @typedef {import('./through').ThroughStream} ThroughStream */

// create a readable writable stream.

/** @type {import('./through')} */
function through(write, end, opts) {
	var writeBound = callBind(write || function (data) { this.queue(data); });
	var endBound = callBind(end || function () { this.queue(null); });

	var ended = false;
	var destroyed = false;
	/** @type {unknown[]} */ var buffer = [];
	var _ended = false;
	// @ts-expect-error
	/** @type {ThroughStream} */ var stream = new Stream();
	stream.readable = true;
	stream.writable = true;
	stream.paused = false;

	// stream.autoPause = !(opts && opts.autoPause === false)
	stream.autoDestroy = !(opts && opts.autoDestroy === false);

	stream.write = function (data) {
		writeBound(this, data);
		return !stream.paused;
	};

	function drain() {
		while (buffer.length && !stream.paused) {
			var data = buffer.shift();
			if (data === null) { return stream.emit('end'); }
			stream.emit('data', data);
		}
	}

	stream.queue = function (data) {
		// console.error(ended)
		if (_ended) { return stream; }
		if (data === null) { _ended = true; }
		buffer.push(data);
		drain();
		return stream;
	};
	stream.push = stream.queue;

	/*
	 * this will be registered as the first 'end' listener must call destroy next tick, to make sure we're after any stream piped from here.
	 * this is only a problem if end is not emitted synchronously.
	 * a nicer way to do this is to make sure this is the last listener for 'end'
	 */

	stream.on('end', function () {
		stream.readable = false;
		if (!stream.writable && stream.autoDestroy) {
			process.nextTick(function () {
				stream.destroy();
			});
		}
	});

	function _end() {
		stream.writable = false;
		endBound(stream);
		if (!stream.readable && stream.autoDestroy) { stream.destroy(); }
	}

	stream.end = function (data) {
		if (ended) { return; }
		ended = true;
		if (arguments.length) { stream.write(data); }
		_end(); // will emit or queue
		return stream;
	};

	stream.destroy = function () {
		if (destroyed) { return; }
		destroyed = true;
		ended = true;
		buffer.length = 0;
		stream.writable = false;
		stream.readable = false;
		stream.emit('close');
		return stream;
	};

	stream.pause = function () {
		if (stream.paused) { return; }
		stream.paused = true;
		return stream;
	};

	stream.resume = function () {
		if (stream.paused) {
			stream.paused = false;
			stream.emit('resume');
		}
		drain();
		// may have become paused again, as drain emits 'data'.
		if (!stream.paused) { stream.emit('drain'); }
		return stream;
	};
	return stream;
}

/*
 * through
 *
 * a stream that does nothing but re-emit the input.
 * useful for aggregating a series of changing but not ending streams into one stream)
 */

module.exports = through;
through.through = through;
