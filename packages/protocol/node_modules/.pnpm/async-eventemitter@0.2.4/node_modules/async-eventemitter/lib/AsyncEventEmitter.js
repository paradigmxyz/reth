'use strict';

var EventEmitter = require('events').EventEmitter,
    util = require('util'),
    eachSeries = require('async/eachSeries'),
    AsyncEventEmitter;


module.exports = exports = AsyncEventEmitter = function AsyncEventEmitter () {
  EventEmitter.call(this);
};

util.inherits(AsyncEventEmitter, EventEmitter);


/* Public methods
============================================================================= */

AsyncEventEmitter.prototype.emit = function(event, data, callback) {
  var self = this,
      listeners = self._events[event] || [];

  // Optional data argument
  if(!callback && typeof data === 'function') {
    callback = data;
    data = undefined;
  }

  // Special treatment of internal newListener and removeListener events
  if(event === 'newListener' || event === 'removeListener') {
    data = {
      event: data,
      fn: callback
    };

    callback = undefined;
  }

  // A single listener is just a function not an array...
  listeners = Array.isArray(listeners) ? listeners : [listeners];

  eachSeries(listeners.slice(), function (fn, next) {
    var err;

    // Support synchronous functions
    if(fn.length < 2) {
      try {
        fn.call(self, data);
      }
      catch (e) {
        err = e;
      }

      return next(err);
    }

    // Async
    fn.call(self, data, next);
  }, callback);

  return self;
};


AsyncEventEmitter.prototype.once = function (type, listener) {
  var self = this,
      g;

  if (typeof listener !== 'function') {
    throw new TypeError('listener must be a function');
  }

  // Hack to support set arity
  if(listener.length >= 2) {
    g = function (e, next) {
      self.removeListener(type, g);
      listener(e, next);
    };
  }
  else {
    g = function (e) {
      self.removeListener(type, g);
      listener(e);
    };
  }

  g.listener = listener;
  self.on(type, g);

  return self;
};


AsyncEventEmitter.prototype.first = function(event, listener) {
  var listeners = this._events[event] || [];

  // Contract
  if(typeof listener !== 'function') {
    throw new TypeError('listener must be a function');
  }

  // Listeners are not always an array
  if(!Array.isArray(listeners)) {
    this._events[event] = listeners = [listeners];
  }

  listeners.unshift(listener);

  return this;
};


AsyncEventEmitter.prototype.at = function(event, index, listener) {
  var listeners = this._events[event] || [];

  // Contract
  if(typeof listener !== 'function') {
    throw new TypeError('listener must be a function');
  }
  if(typeof index !== 'number' || index < 0) {
    throw new TypeError('index must be a non-negative integer');
  }

  // Listeners are not always an array
  if(!Array.isArray(listeners)) {
    this._events[event] = listeners = [listeners];
  }

  listeners.splice(index, 0, listener);

  return this;
};


AsyncEventEmitter.prototype.before = function(event, target, listener) {
  return this._beforeOrAfter(event, target, listener);
};


AsyncEventEmitter.prototype.after = function(event, target, listener) {
  return this._beforeOrAfter(event, target, listener, 'after');
};


/* Private methods
============================================================================= */

AsyncEventEmitter.prototype._beforeOrAfter = function(event, target, listener, beforeOrAfter) {
  var listeners = this._events[event] || [],
      i, index,
      add = beforeOrAfter === 'after' ? 1 : 0;

  // Contract
  if(typeof listener !== 'function') {
    throw new TypeError('listener must be a function');
  }
  if(typeof target !== 'function') {
    throw new TypeError('target must be a function');
  }

  // Listeners are not always an array
  if(!Array.isArray(listeners)) {
    this._events[event] = listeners = [listeners];
  }

  index = listeners.length;
  
  for(i = listeners.length; i--;) {
    if(listeners[i] === target) {
      index = i + add;
      break;
    }
  }

  listeners.splice(index, 0, listener);

  return this;
};
