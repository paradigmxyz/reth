
var looper = module.exports = function (fun) {
  return function next (a, b, c) {
    var loop = true, returned = false, sync = false
    do {
      sync = true; loop = false
      fun.call(function (x, y, z) {
        if(sync) {
          a = x; b = y; c = z
          loop = true
        }
        else
          next(x, y, z)
      }, a, b, c)
      sync = false
    } while(loop)
  }
}
