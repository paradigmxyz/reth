'use strict';

var resumer = require('../');

function createStream() {
	var stream = resumer();
	stream.queue('beep boop\n');
	return stream;
}

createStream().pipe(process.stdout);
