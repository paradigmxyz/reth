var ltgt = require('ltgt')

function eq (a, b) {
  if (Buffer.isBuffer(a) && Buffer.isBuffer(b)) {
    return beq(a, b)
  }
  else return a === b
}

module.exports = function post (db, opts, each) {
  if(!each)
    each = opts, opts = {}

  if('function' === typeof db.post)
    return db.post(opts, each)

  var encode = (opts && opts.keyEncoding && opts.keyEncoding.encode)
    || (db.options && db.options.keyEncoding && db.options.keyEncoding.encode)
    || function (x) { return x }

  var _opts = ltgt.toLtgt(opts, {}, encode)

  function cmp (key) {
    return ltgt.contains(_opts, encode(key))
  }

  function onPut (key, val) {
    if(cmp(key))
      each({type: 'put', key: key, value: val})
  }

  function onDel (key, val) {
    if(cmp(key))
      each({type: 'del', key: key, value: val})
  }

  function onBatch (ary) {
    ary.forEach(function (op) {
      if(cmp(op.key))
        each(op)
    })
  }

  db.on('put', onPut)
  db.on('del', onDel)
  db.on('batch', onBatch)

  return function () {
    db.removeListener('put', onPut)
    db.removeListener('del', onDel)
    db.removeListener('batch', onBatch)
  }
}

