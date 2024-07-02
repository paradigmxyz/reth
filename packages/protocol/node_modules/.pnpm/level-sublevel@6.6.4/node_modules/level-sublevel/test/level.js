var tape = require('tape')
var pull = require('pull-stream')
var path = require('path')

//the mock is partical levelup api.
var mock  = require('./mock')
var nut   = require('../nut')
var shell = require('../shell') //the shell surrounds the nut
var Codec = require('level-codec')
var concat = require('../codec')
var legacy = require('../codec/legacy')
var bytewise = require('../codec/bytewise')


var codex = [
  concat,
  legacy,
  bytewise
]

var pullReadStream = require('../pull')

function create (precodec, db) {

  //convert pull stream to iterators
  return shell ( nut ( db || mock(), precodec, new Codec ), [], pullReadStream)
}

function prehookPut (db) {
  tape('test - prehook - put', function (t) {

    var log = db.sublevel('log')
    var c = 0
    db.pre(function (op, add) {
      add({key: ''+c++, value: op.key, prefix: log.prefix()})
    })

    db.put('hello', 'there?', function (err) {

      if(err) throw err
      log.get('0', function (err, value) {
        if(err) throw err
        t.equal(value, 'hello')
        t.end()
      })
    })
  })
}

function prehookBatch (db) {
  tape('test - prehook - put', function (t) {
//    var db = shell ( nut ( mock(), precodec, codec ) )

    var log = db.sublevel('log')
    var c = 0
    db.pre(function (op, add) {
      add({key: ''+c++, value: op.key, prefix: log.prefix()})
    })

    db.batch([
      {key:'hello1', value: 'there.', type: 'put'},
      {key:'hello2', value: 'there!', type: 'put'},
      {key:'hello3', value: 'where?', type: 'put'},
    ], function (err) {
      if(err) throw err
      log.get('0', function (err, value) {
        if(err) throw err
        t.equal(value, 'hello1')
        log.get('1', function (err, value) {
          if(err) throw err
          t.equal(value, 'hello2')
          log.get('2', function (err, value) {
            if(err) throw err
            t.equal(value, 'hello3')
            t.end()
          })
        })
      })
    })
  })
}

function createPostHooks (db) {

  function posthook (args, calls, db) {
    //db = db || shell ( nut ( mock(), concat, codec ) )

    var method = args.shift()
    tape('test - posthook - ' + method, function (t) {

      var cb = 0, hk = 0
      var rm = db.post(function (op) {
        hk ++
        next()
      })
  
      console.log(db, args)
      db[method].apply(db, args.concat(function (err) {
        console.log('**************8')
        if(err) console.log(err.stack)
//        if(err) throw err
        cb ++
        next()
      }))

      function next () {
        if(cb + hk < calls + 1) return
        t.equal(cb, 1)
        t.equal(hk, calls)
        rm()
        t.end()
      }

    })
  }

  // test posthooks trigger correct number of times


  posthook(['put', 'hello', 'there?'], 1, db)
  posthook(['del', 'hello'], 1, db)
  posthook(['batch', [
    { key: 'foo', value: 'bar', type: 'put'},
    { key: 'fuz', value: 'baz', type: 'put'},
    { key: 'fum', value: 'boo', type: 'put'}
    ]], 3, db)

}

// test posthooks also work in sublevels

//test removing hooks.

function rmHook (db) {
  tape('test - prehook - put', function (t) {
//    db = db || shell ( nut ( mock(), precodec, codec ) )

    var hk = 0
    var rm = db.pre(function (op, add) {
      hk ++
      t.equal(op.key, 'hello')
      t.equal(op.value, 'there')
    })

    db.put('hello', 'there', function (err) {
      if(err) throw err
      t.equal(hk, 1)
      db.put('hello', 'where?', function (err) {
        if(err) throw err
        t.equal(hk, 1)
        t.end()
      })
    })
    rm()

  })
  
}

function stream (db) {

  tape('pull-stream', function (t) {  
    var batch = [
      { key: 'foo', value: 'bar'},
      { key: 'fum', value: 'boo'},
      { key: 'fuz', value: 'baz'}
    ]

    db.batch(batch, function (err) {
      if(err) throw err

      pull(db.createReadStream(), pull.collect(function (err, ary) {
        if(err) throw err
        console.log(ary)
        t.deepEqual(ary, batch)
        t.end()
      }))
    })
  })
}


var tests = [
  prehookPut,
  prehookBatch,
  createPostHooks,
  rmHook,
  stream
]

var LevelDown = require('leveldown')
var i = 0
var rimraf = require('rimraf')

function createTestDb () {
  var dir = path.join('/tmp', 'level-sublevel_test' + (i++))
  rimraf.sync(dir)
  return new LevelDown(dir)
}

codex.forEach(function (codec) {

  tests.forEach(function (test) {

    var db1 = create(codec)

    test(db1)
    test(db1.sublevel('foo'))
    test(db1.sublevel('foo').sublevel('blah'))

    var db3 = create(codec, createTestDb())

    test(db3)
    test(db3.sublevel('foo'))
    test(db3.sublevel('foo').sublevel('blah'))

  })

})

