var test = require('tape');
var through = require('through');

var sublevel = require('../');
var level = require('level-test')();

var db = level('key-stream-alias');
var sub = sublevel(db).sublevel('test');

function streamToArray(stream, cb) {
  var arr = [];
  stream.pipe(through(arr.push.bind(arr), function (err) {
    return cb(err, arr);
  }));
}
function testMethodsOnDb(db, name, alias, cb) {
  db.batch([
    {key: 'a', value: 1, type: 'put'},
    {key: 'b', value: 2, type: 'put'},
    {key: 'c', value: 3, type: 'put'},
  ], function (err) {

    streamToArray(db[name](), next);

    function next(err, arr1) {
      if (err) { return cb(err); }
      streamToArray(db[alias](), function (err, arr2) {
        return cb(err, arr1, arr2);
      });
    }
  });

}

test('keyStream/createKeyStream', function (t) {
  testMethodsOnDb(db, 'keyStream', 'createKeyStream', function (err, arr1, arr2) {
    t.notOk(err);
    t.same(arr1, arr2);
    t.end();
  });
});

test('readStream/createReadStream', function (t) {
  testMethodsOnDb(db, 'readStream', 'createReadStream', function (err, arr1, arr2) {
    t.notOk(err);
    t.same(arr1, arr2);
    t.end();
  });
});

test('valueStream/createValueStream', function (t) {
  testMethodsOnDb(db, 'valueStream', 'createValueStream', function (err, arr1, arr2) {
    t.notOk(err);
    t.same(arr1, arr2);
    t.end();
  });
});

test('sublevel keyStream/createKeyStream', function (t) {
  testMethodsOnDb(sub, 'keyStream', 'createKeyStream', function (err, arr1, arr2) {
    t.notOk(err);
    t.same(arr1, arr2);
    t.end();
  });
});

test('sublevel readStream/createReadStream', function (t) {
  testMethodsOnDb(sub, 'readStream', 'createReadStream', function (err, arr1, arr2) {
    t.notOk(err);
    t.same(arr1, arr2);
    t.end();
  });
});

test('sublevel valueStream/createValueStream', function (t) {
  testMethodsOnDb(sub, 'valueStream', 'createValueStream', function (err, arr1, arr2) {
    t.notOk(err);
    t.same(arr1, arr2);
    t.end();
  });
});

