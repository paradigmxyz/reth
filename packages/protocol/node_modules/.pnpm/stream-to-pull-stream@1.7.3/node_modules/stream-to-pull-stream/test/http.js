var http   = require('http')
var fs     = require('fs')
var toPull = require('../')
var pull   = require('pull-stream')
var port   = ~~(Math.random()*40000) + 1024
var test   = require('tape')

var thisFile = fs.readFileSync(__filename, 'utf-8')

test('test http', function (t) {

  var server = http.createServer(function (req, res) {
    pull(
      toPull(req),
      pull.reduce(function (b, s) {
          return b + s
        }, '', function (err, body) {
          t.equal(body, thisFile)
          t.notOk(err)
          res.end('done')
      })
    )
  }).listen(port, function () {

    fs.createReadStream(__filename)
      .pipe(http.request({method: 'PUT', port: port}, function (res) {
        console.log(res.statusCode)
        var _res = toPull(res)

        setTimeout(function () {

          pull(
            _res,
            pull.collect(function (err, ary) {
              t.equal(ary.map(String).join(''), 'done')
              t.end()
            })
          )

        }, 200)

        server.close()
      }))
  })

})
