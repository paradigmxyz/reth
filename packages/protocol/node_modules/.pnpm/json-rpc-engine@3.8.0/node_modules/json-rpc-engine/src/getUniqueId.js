// uint32 (two's complement) max
// more conservative than Number.MAX_SAFE_INTEGER
const MAX = 4294967295
let idCounter = Math.floor(Math.random() * MAX)

module.exports = getUniqueId

function getUniqueId () {
  idCounter = (idCounter + 1) % MAX
  return idCounter
}