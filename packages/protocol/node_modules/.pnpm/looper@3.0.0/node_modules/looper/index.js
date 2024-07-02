
var looper = module.exports = function (fun) {
  (function next () {
    var loop = true, returned = false, sync = false
    do {
      sync = true; loop = false
      fun.call(this, function () {
        if(sync) loop = true
        else     next()
      })
      sync = false
    } while(loop)
  })()
}
