var should = require('should');
var assert = require('assert');
var semaphore = require("../lib/semaphore.js");
require('mocha');

var Phone = function() {
	return {
		state: "free",

		dial: function(callback) {
			if (this.state != "free") {
				return callback(new Error("The phone is busy"));
			}

			this.state = "busy";

			setTimeout(function() {
				callback();
			}, 100);
		},

		hangup: function() {
			if (this.state == "free") {
				return callback(new Error("The phone is not in use"));
			}

			this.state = "free";
		}
	};
};

it("should not be using a bad example", function(done) {
	var phone = new Phone();

	// Call Bob
	phone.dial(function(err) {
		if (err) return done(err);

		phone.hangup();
	});

	// Cannot call Bret, because the phone is already busy with Bob.
	phone.dial(function(err) {
		should.exist(err);
		done();
	});
});

it("should not break the phone", function(done) {
	var phone = new Phone();
	var sem = require('../lib/semaphore.js')(1);

	// Call Jane
	sem.take(function() {
		phone.dial(function(err) {
			if (err) return done(err);

			phone.hangup();

			sem.leave();
		});
	});

	// Call Jon (will need to wait for call with Jane to complete)
	sem.take(function() {
		phone.dial(function(err) {
			if (err) return done(err);

			phone.hangup();

			sem.leave();

			done();
		});
	});
});

it('should not be slow', function(done) {
	var s = require('../lib/semaphore.js')(3);
	var values = [];

	s.take(function() { values.push(1); s.leave(); });
	s.take(function() { values.push(2); s.leave(); });
	s.take(function() { values.push(3); s.leave(); });
	s.take(function() { values.push(4); s.leave(); });
	s.take(function() { values.push(5); s.leave(); });

	process.nextTick(function() {
		values.length.should.equal(5);
		done();
	});
});

it('should not let past more than capacity', function(done) {
	this.timeout(6000);

	var s = require('../lib/semaphore.js')(3);
	var values = [];
	var speed = 250;

	s.take(function() { values.push(1); setTimeout(function() { s.leave(); }, speed * 1); });
	s.take(function() { values.push(2); setTimeout(function() { s.leave(); }, speed * 2); });
	s.take(function(leave) { values.push(3); setTimeout(function() { leave(); }, speed * 3); });
	s.take(function() { values.push(4); });
	s.take(function() { values.push(5); });

	var tickN = 0;

	var check = function() {
		switch (tickN++) {
			case 0: // After 0 sec
				console.log("0 seconds passed.")
				s.current.should.equal(s.capacity);
				s.queue.length.should.equal(2);
				values.should.eql([1, 2, 3]);
				break;
			case 1: // After 1 sec
				console.log("1 seconds passed.");
				s.current.should.equal(s.capacity);
				s.queue.length.should.equal(1);
				values.should.eql([1, 2, 3, 4]);
				break;
			case 2: // After 2 sec
				console.log("2 seconds passed.");
				s.current.should.equal(3);
				s.queue.length.should.equal(0);
				values.should.eql([1, 2, 3, 4, 5]);
				break;
			case 3: // After 3 sec
				console.log("3 seconds passed.");
				s.current.should.equal(2);
				s.queue.length.should.equal(0);
				values.should.eql([1, 2, 3, 4, 5]);
				return done();
		}

		setTimeout(check, speed * 1.1);
	};

	check();
});

describe("should respect number", function() {
	it("should fail when taking more than the capacity allows", function(done) {
		var s = semaphore(1);

		s.take(2, function() {
			assert.fail();
		});

		process.nextTick(done);
	});

	it("should work fine with correct input values", function(done) {
		var s = semaphore(10); // 10

		s.take(5, function(leave) { // 5
			s.take(4, function() { // 1
				leave(4); // 5

				s.take(5, function() {
					return done()
				}); // 0
			});
		});
	});
});
