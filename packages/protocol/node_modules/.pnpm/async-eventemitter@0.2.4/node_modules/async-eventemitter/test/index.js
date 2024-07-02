'use strict';

/*global it:true, describe:true*/
/*jshint unused:false*/
var should = require('should'),
    AsyncEventEmitter = require('../.'),
    events,
    i;

describe('An instance', function () {
  it('should be created', function () {
    events = new AsyncEventEmitter();
  });
});

describe('on()', function () {
  function listener1 (e, callback) {
    // Ensure context is kept
    /* jshint validthis:true */
    this.should.equal(events);

    setTimeout(function () {
      i++;

      (typeof e).should.equal('object');
      (typeof callback).should.equal('function');

      callback();
    });
  }

  function listener2 (e, callback) {
    setTimeout(function () {
      i++;

      (typeof e).should.equal('object');
      (typeof callback).should.equal('function');

      callback();
    });
  }

  it('should register an eventlistener', function () {
    events.on('test1', listener1);
    events.on('test1', listener2);
    events._events.should.have.property('test1');
  });
});

describe('emit()', function () {
  it('should emit event and call callback after all eventlisteners are done', function (done) {
    i = 0;

    events.emit('test1', {}, function (err) {
      i.should.equal(2);
      done();
    });
  });

  it('should emit with no argument', function (done) {
    events.on('no-arg', function (e, next) {
      (typeof e).should.equal('undefined');
      next();
      done();
    });

    events.emit('no-arg');
  });

  it('should emit with only data argument', function (done) {
    events.on('data-only', function (e, next) {
      e.should.equal(1);
      next();
      done();
    });

    events.emit('data-only', 1);
  });

  it('should emit with only callback argument', function (done) {
    events.on('function-only', function (e, next) {
      (typeof e).should.equal('undefined');
      next();
    });

    events.emit('function-only', done);
  });
});

describe('eventlisteners', function () {
  it('should be synchronous if no next-argument specified', function (done) {
    events.on('sync', function (e) {
      e.should.equal(1);
    });

    events.emit('sync', 1, done);
  });
});

describe('next(err)', function () {
  it('should abort the callback chain', function (done) {
    events.on('err', function (e, next) {
      next(1);
    });

    events.on('err', function (e, next) {
      throw('Expected this function to not be called');
    });

    events.emit('err', function (err) {
      err.should.equal(1);
      done();
    });
  });
});

describe('newListener-events', function () {
  // Use separate test-object to not break other tests
  var events = new AsyncEventEmitter();

  it('should supply the event listener as e and not next', function (done) {
    function newListener (e) {
      e.should.have.property('event').and.equal('newListener-test');
      e.should.have.property('fn').and.equal(test);
      done();
    }

    function test () {}

    events.on('newListener', newListener);
    events.on('newListener-test', test);
  });
});

describe('removeListener-events', function () {
  var events = new AsyncEventEmitter();

  it('should supply the event listener as e and not next', function (done) {
    function removeListener (e) {
      e.should.have.property('event').and.equal('test');
      e.should.have.property('fn').and.equal(test);
      done();
    }

    function test () {}

    events.on('removeListener', removeListener);
    events.on('test', test);
    events.removeListener('test', test);
  });
});

describe('once()', function () {
  var i = 0;

  function listener1 (e, callback) {
    setTimeout(function () {
      i++;
      callback();
    });
  }

  it('should register eventlisteners', function () {
    events.once('test-once', listener1);
    events._events.should.have.property('test-once');
  });

  describe('eventlisteners', function () {
    it('should only be called once', function (done) {
      events.emit('test-once', function () {
        i.should.equal(1);

        events.emit('test-once', function () {
          i.should.equal(1);
          done();
        });
      });
    });
  });
});

describe('removeAllListeners', function () {
  var events = new AsyncEventEmitter();

  events.on('test', function () {});
  events.on('test2', function () {});

  describe('(event)', function () {
    it('should remove all event listeners for event', function () {
      events._events.should.have.property('test');
      events.removeAllListeners('test');
      events._events.should.not.have.property('test');
    });
  });

  describe('()', function () {
    it('should remove all event listeners for all events', function () {
      events._events.should.have.property('test2');
      events.removeAllListeners();
      events._events.should.not.have.property('test2');
    });
  });
});

describe('listeners()', function () {
  var events = new AsyncEventEmitter();

  function test () {}

  events.on('test', test);

  it('should return all listeners for the specified event', function () {
    var listeners = events.listeners('test');

    listeners.should.have.property('length').and.equal(1);
    listeners[0].should.equal(test);
  });
});

describe('all overriden methods', function () {
  var events = new AsyncEventEmitter();

  describe('(.on())', function () {
    it('should be chainable', function () {
      events.on('test', function () {}).should.be.instanceOf(AsyncEventEmitter);
    });
  });

  describe('(.once())', function () {
    it('should be chainable', function () {
      events.once('test', function () {}).should.be.instanceOf(AsyncEventEmitter);
    });
  });

  describe('(.emit())', function () {
    it('should be chainable', function () {
      events.emit('test').should.be.instanceOf(AsyncEventEmitter);
    });
  });
});

describe('all added methods', function () {
  var events = new AsyncEventEmitter();

  describe('(.first())', function () {
    it('should be chainable', function () {
      events.first('test', function () {}).should.be.instanceOf(AsyncEventEmitter);
    });
  });

  describe('(.at())', function () {
    it('should be chainable', function () {
      events.at('test', 1, function () {}).should.be.instanceOf(AsyncEventEmitter);
    });
  });

  describe('(.before())', function () {
    it('should be chainable', function () {
      events.before('test', function () {}, function () {}).should.be.instanceOf(AsyncEventEmitter);
    });
  });

  describe('(.after())', function () {
    it('should be chainable', function () {
      events.after('test', function () {}, function () {}).should.be.instanceOf(AsyncEventEmitter);
    });
  });
});

describe('first()', function () {
  var events = new AsyncEventEmitter();

  function test () {}

  it('should add an event listener first in the chain', function () {
    events.on('test', function () {});
    events.first('test', test);
    events._events.test[0].should.equal(test);
    events._events.test.length.should.equal(2);
  });
});

describe('at()', function () {
  var events = new AsyncEventEmitter();

  function test () {}

  it('should insert an event listener at the specified index', function () {
    events.on('test', function () {});
    events.on('test', function () {});
    events.on('test', function () {});
    events.at('test', 2, test);

    events._events.test[2].should.equal(test);
    events._events.test.length.should.equal(4);
  });

  it('should push a listener if the index is larger than the length', function () {
    events.at('test', 10, test);

    events._events.test[events._events.test.length - 1].should.equal(test);
    events._events.test.length.should.equal(5);
  });
});

describe('before()', function () {
  var events = new AsyncEventEmitter();

  function target () {}
  function listener () {}

  it('should insert a listener before the specified target', function () {
    events.on('test', function () {});
    events.on('test', target);
    events.before('test', target, listener);

    events._events.test[1].should.equal(listener);
    events._events.test.length.should.equal(3);
  });

  it('should push a listener if the target is not found', function () {
    events.on('test2', function () {});
    events.before('test2', target, listener);

    events._events.test2[1].should.equal(listener);
    events._events.test2.length.should.equal(2);
  });
});

describe('after()', function () {
  var events = new AsyncEventEmitter();

  function target () {}
  function listener () {}

  it('should insert a listener after the specified target', function () {
    events.on('test', function () {});
    events.on('test', target);
    events.after('test', target, listener);

    events._events.test[2].should.equal(listener);
    events._events.test.length.should.equal(3);
  });

  it('should push a listener if the target is not found', function () {
    events.on('test2', function () {});
    events.after('test2', target, listener);

    events._events.test2[1].should.equal(listener);
    events._events.test2.length.should.equal(2);
  });
});

describe('Sync listener returning an Error', function () {
  function err () {
    throw new Error('Die!');
  }

  it('should abort the listener chain', function (done) {
    events.on('errorTest', err);
    events.on('errorTest', function () {
      // Just make sure this is never run
      true.should.equal(false);
    });

    events.emit('errorTest', function (err) {
      err.should.be.instanceOf(Error);
      done();
    });
  });
});
