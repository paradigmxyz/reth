var Live     = require('pull-live')

exports.old = require('./old')
exports.live = require('./live')


exports.read =
exports.readStream =
exports.createReadStream = require('./read')

exports.write =
exports.writeStream =
exports.createWriteStream = require('./write')
