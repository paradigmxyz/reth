'use strict'

class ArgRegExp extends RegExp {
  name (arg) {
    return arg.match(this)[1]
  }
}

exports.short = new ArgRegExp('^-([^\\d-])$')
exports.long = new ArgRegExp('^--(\\S+)')
exports.combined = new ArgRegExp('^-([^\\d-]{2,})$')
exports.isOption = arg => exports.short.test(arg) || exports.long.test(arg)
exports.optEquals = new ArgRegExp('^(--\\S+?)=(.*)')
exports.VALUE_MARKER = '552f3a31-14cd-4ced-bd67-656a659e9efb' // must be unique
