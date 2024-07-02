const https = require('https')
const createConcatStream = require('mississippi').concat
const RpcBlockTracker = require('./lib/index')



const provider = { sendAsync }
const blockTracker = new RpcBlockTracker({ provider })
blockTracker.start()

blockTracker.on('block', (newBlock) => {
  console.log(`block #${Number(newBlock.number)}`)
})

blockTracker.on('sync', ({ newBlock, oldBlock }) => {
  if (oldBlock) {
    console.log(`sync #${Number(oldBlock.number)} -> #${Number(newBlock.number)}`)
  } else {
    console.log(`first sync #${Number(newBlock.number)}`)
  }
})


function sendAsync(payload, cb){
  const data = new Buffer(JSON.stringify(payload))
  const req = https.request({
    host: 'mainnet.infura.io',
    method: 'POST',
  }, (res) => {
    res.setEncoding('utf8')
    res.pipe(createConcatStream((result) => cb(null, JSON.parse(result))))
    res.on('error', cb)
  })
  req.write(data)
  req.end()
}

