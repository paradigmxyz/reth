const getUniqueId = require('./getUniqueId')

module.exports = createIdRemapMiddleware

function createIdRemapMiddleware() {
  return (req, res, next, end) => {
    const originalId = req.id
    const newId = getUniqueId()
    req.id = newId
    res.id = newId
    next((done) => {
      req.id = originalId
      res.id = originalId
      done()
    })
  }
}
