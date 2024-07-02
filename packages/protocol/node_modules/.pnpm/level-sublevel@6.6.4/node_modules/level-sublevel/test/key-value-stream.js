
var pl   = require('pull-level')
var pull = require('pull-stream')
var toPull = require('stream-to-pull-stream')

var level = require('level-test')()
var sublevel = require('../')
var tape = require('tape')

tape('keys', function (t) {

  var db = sublevel(level()).sublevel('test')

  pull(
    pull.count(10),
    pull.map(function (i) {
      return {key: 'key_'+i, value: 'value_' + i}
    }),
    pl.write(db, function (err) {
      if(err) {
        t.notOk(err)
        throw err
      }

      pull(
        toPull(db.createKeyStream()),
        pull.collect(function (err, ary) {
          console.log(ary)
          ary.forEach(function (e) {
            t.equal(typeof e, 'string')
            t.ok(/^key_/.test(e))
          })
          pull(
            toPull(db.createValueStream()),
            pull.collect(function (err, ary) {
              console.log(ary)
              ary.forEach(function (e) {
                t.equal(typeof e, 'string')
                t.ok(/^value_/.test(e))
                console.log(e)
              })

              t.end()
            })
          )
        })
      )
    })
  )
})

