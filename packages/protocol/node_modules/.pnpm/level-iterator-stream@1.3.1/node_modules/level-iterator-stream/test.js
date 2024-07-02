var test = require('tape');
var leveldown = require('leveldown');
var iteratorStream = require('./');
var through2 = require('through2');
var abstract = require('abstract-leveldown');
var Codec = require('level-codec');
var EncodingError = require('level-errors').EncodingError;

var db;
var data = [
  { type: 'put', key: 'foobatch1', value: 'bar1' },
  { type: 'put', key: 'foobatch2', value: 'bar2' },
  { type: 'put', key: 'foobatch3', value: 'bar3' }
];

test('setup', function(t){
  db = leveldown(__dirname + '/db-test');
  db.open(function(err){
    t.error(err);
    db.batch(data, function(err){
      t.error(err);
      t.end();
    });
  });
});

test('simple', function(t){
  var idx = 0;
  var stream = iteratorStream(db.iterator());
  stream.pipe(through2.obj(function(kv, _, done){
    t.ok(Buffer.isBuffer(kv.key));
    t.ok(Buffer.isBuffer(kv.value));
    t.equal(kv.key.toString(), data[idx].key);
    t.equal(kv.value.toString(), data[idx].value);
    idx++;
    done();
  }, function(){
    t.equal(idx, data.length);
    stream.on('close', function(){
      t.end();
    });
  }));
});

test('destroy', function(t){
  var stream = iteratorStream(db.iterator());
  stream.on('close', t.end.bind(t));
  stream.destroy();
});

test('decoder', function(t){
  var codec = new Codec({ valueEncoding: 'binary' });
  var stream = iteratorStream(db.iterator(), {
    decoder: codec.createStreamDecoder({ values: true })
  });
  stream.once('data', function(value){
    t.ok(Buffer.isBuffer(value));
    t.equal(value.toString(), 'bar1');
    t.end();
  });
});

test('decoder error', function(t){
  t.plan(2);
  var codec = new Codec({ valueEncoding: 'json' });
  var stream = iteratorStream(db.iterator(), {
    decoder: codec.createStreamDecoder({ values: true })
  });
  stream.once('error', function(err){
    t.ok(err instanceof EncodingError);
  });
  stream.once('close', function(){
    t.ok(true);
  });
  stream.on('data', function(){});
});
