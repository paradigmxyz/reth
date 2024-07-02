
//test that stdout doesn't leave anything hanging.
//unfortunately i have not been able to reproduce this automatically.
//so you have to run this and check for valid output + the text "VALID END"


var toPull = require('../')
var pull = require('pull-stream')

pull(
  pull.count(150),
  pull.map(function () {
    return {
      okay: true,
      date: new Date(),
      array: [1,3,4,6,7498,49,837,9],
      nest: {foo:{bar:{baz: null}}},
      pkg: require('../package')
    }
  }),
  pull.map(function (e) {
    return JSON.stringify(e, null, 2) +'\n\n'
  }),
  toPull.sink(process.stdout, function (err) {
    if(err) throw err
//    console.log('VALID END')
    process.exit()
  })
)
