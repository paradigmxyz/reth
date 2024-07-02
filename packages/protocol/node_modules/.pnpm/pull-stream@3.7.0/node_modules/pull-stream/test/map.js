
var tape = require('tape')

var pull = require('../')


tape('map throughs ends stream', function (t) {
  var err = new Error('unwholesome number')
  pull(
    pull.values([1,2,3,3.4,4]),
    pull.map(function (e) {
      if(e !== ~~e)
        throw err
    }),
    pull.drain(null, function (_err) {
      t.equal(_err, err)
      t.end()
    })
  )
})
