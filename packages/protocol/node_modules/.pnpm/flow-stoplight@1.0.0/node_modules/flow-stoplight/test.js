var test = require('tape')
var Stoplight = require('./index.js')

test('basic test', function(t){
  t.plan(3)

  var stoplight = new Stoplight()

  var wasCalled = false

  stoplight.await(function(){
    wasCalled = true
  })

  t.equal(wasCalled, false, 'flow is stopped')

  setTimeout(function(){
    
    t.equal(wasCalled, false, 'flow is still stopped')
    stoplight.go()

    setTimeout(function(){
    
      t.equal(wasCalled, true, 'flow is flowing')
      t.end()

    })

  })

})