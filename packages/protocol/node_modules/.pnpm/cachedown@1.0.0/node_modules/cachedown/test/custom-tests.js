'use strict';

var levelup = require('levelup');
var Storage = require('../').Storage

module.exports.setUp = function (leveldown, test, testCommon) {
  test('setUp common', testCommon.setUp);
  test('setUp db', function (t) {
    var db = leveldown(testCommon.location());
    db.open(t.end.bind(t));
  });
};

module.exports.all = function (leveldown, tape, testCommon) {
  module.exports.setUp(leveldown, tape, testCommon);

  tape('test put/get/del hit cache', function (t) {
    var db = levelup('blah', {db: leveldown}, function () {
      var cache = db.db._cache
      var underlyingDown = db.db._down
      db.put('a', 'b', function () {
        t.equal(cache.get('a'), 'b')
        var get = underlyingDown._get
        underlyingDown._get = function () {
          t.fail('value should come from cache')
          return get.apply(underlyingDown, arguments)
        }

        db.get('a', function (err, val) {
          t.error(err)
          t.equal(val, 'b')
          db.del('a', function () {
            t.equal(cache.get('a'), undefined)
            t.end()
          })
        })
      })

      t.equal(cache.get('a'), 'b')
    })
  })

  tape('test escaped db name', function (t) {
    var db = levelup('bang!', {db: leveldown});
    var db2 = levelup('bang!!', {db: leveldown});
    db.put('!db1', '!db1', function (err) {
      t.notOk(err, 'no error');
      db2.put('db2', 'db2', function (err) {
        t.notOk(err, 'no error');
        db.close(function (err) {
          t.notOk(err, 'no error');
          db2.close(function (err) {
            t.notOk(err, 'no error');
            db = levelup('bang!', {db: leveldown});
            db.get('!db2', function (err, key, value) {
              t.ok(err, 'got error');
              t.equal(key, undefined, 'key should be null');
              t.equal(value, undefined, 'value should be null');
              t.end();
            });
          });
        });
      });
    });
  });

  tape('delete while iterating', function (t) {
    var db = leveldown(testCommon.location());
    var noerr = function (err) {
      t.error(err, 'opens crrectly');
    };
    var noop = function () {};
    var iterator;
    db.open(noerr);
    db.put('a', 'A', noop);
    db.put('b', 'B', noop);
    db.put('c', 'C', noop);
    iterator = db.iterator({ keyAsBuffer: false, valueAsBuffer: false, start: 'a' });
    iterator.next(function (err, key, value) {
      t.equal(key, 'a');
      t.equal(value, 'A');
      db.del('b', function (err) {
        t.notOk(err, 'no error');
        iterator.next(function (err, key, value) {
          t.notOk(err, 'no error');
          t.ok(key, 'key exists');
          t.ok(value, 'value exists');
          t.end();
        });
      });
    });
  });

  tape('add many while iterating', function (t) {
    var db = leveldown(testCommon.location());
    var noerr = function (err) {
      t.error(err, 'opens crrectly');
    };
    var noop = function () {};
    var iterator;
    db.open(noerr);
    db.put('c', 'C', noop);
    db.put('d', 'D', noop);
    db.put('e', 'E', noop);
    iterator = db.iterator({ keyAsBuffer: false, valueAsBuffer: false, start: 'c' });
    iterator.next(function (err, key, value) {
      t.equal(key, 'c');
      t.equal(value, 'C');
      db.del('c', function (err) {
        t.notOk(err, 'no error');
        db.put('a', 'A', function (err) {
          t.notOk(err, 'no error');
          db.put('b', 'B', function (err) {
            t.notOk(err, 'no error');
            iterator.next(function (err, key, value) {
              t.notOk(err, 'no error');
              t.ok(key, 'key exists');
              t.ok(value, 'value exists');
              t.ok(key >= 'c', 'key "' + key + '" should be greater than c');
              t.end();
            });
          });
        });
      });
    });
  });

  tape('concurrent batch delete while iterating', function (t) {
    var db = leveldown(testCommon.location());
    var noerr = function (err) {
      t.error(err, 'opens crrectly');
    };
    var noop = function () {};
    var iterator;
    db.open(noerr);
    db.put('a', 'A', noop);
    db.put('b', 'B', noop);
    db.put('c', 'C', noop);
    iterator = db.iterator({ keyAsBuffer: false, valueAsBuffer: false, start: 'a' });
    iterator.next(function (err, key, value) {
      t.equal(key, 'a');
      t.equal(value, 'A');
      db.batch([{
        type: 'del',
        key: 'b'
      }], noerr);
      iterator.next(function (err, key, value) {
        t.notOk(err, 'no error');
        // on backends that support snapshots, it will be 'b'.
        // else it will be 'c'
        t.ok(key, 'key should exist');
        t.ok(value, 'value should exist');
        t.end();
      });
    });
  });

  tape('iterate past end of db', function (t) {
    var db = leveldown('aaaaaa');
    var db2 = leveldown('bbbbbb');
    var noerr = function (err) {
      t.error(err, 'opens crrectly');
    };
    var noop = function () {};
    var iterator;
    db.open(noerr);
    db2.open(noerr);
    db.put('1', '1', noop);
    db.put('2', '2', noop);
    db2.put('3', '3', noop);
    iterator = db.iterator({ keyAsBuffer: false, valueAsBuffer: false, start: '1' });
    iterator.next(function (err, key, value) {
      t.equal(key, '1');
      t.equal(value, '1');
      t.notOk(err, 'no error');
      iterator.next(function (err, key, value) {
        t.notOk(err, 'no error');
        t.equals(key, '2');
        t.equal(value, '2');
        iterator.next(function (err, key, value) {
          t.notOk(key, 'should not actually have a key');
          t.end();
        });
      });
    });
  });
};
