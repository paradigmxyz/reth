'use strict';

var collect = require('./collect')

module.exports = function collectAsPromise() {
  return (source) => {
    let resolve
    let reject
    const promise = new Promise((res, rej) => {
      resolve = res
      reject = rej
    });

    collect((err, ary) => {
      if (err) reject(err)
      else resolve(ary)
    })(source)

    return promise
  }
}
