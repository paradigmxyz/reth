const promiseToCallback = require('promise-to-callback')

module.exports = createAsyncMiddleware


function createAsyncMiddleware(asyncMiddleware) {
  return (req, res, next, end) => {
    let nextDonePromise = null
    const finishedPromise = asyncMiddleware(req, res, getNextPromise)
    promiseToCallback(finishedPromise)((err) => {
      // async middleware ended
      if (nextDonePromise) {
        // next handler was called - complete nextHandler
        promiseToCallback(nextDonePromise)((nextErr, nextHandlerSignalDone) => {
          // nextErr is only present if something went really wrong
          // if an error is thrown after `await next()` it appears as `err` and not `nextErr`
          if (nextErr) {
            console.error(nextErr)
            return end(nextErr)
          }
          nextHandlerSignalDone(err)
        })
      } else {
        // next handler was not called - complete middleware
        end(err)
      }
    })

    async function getNextPromise() {
      nextDonePromise = getNextDoneCallback()
      await nextDonePromise
      return undefined
    }

    function getNextDoneCallback() {
      return new Promise((resolve) => {
        next((cb) => resolve(cb))
      })
    }
  }
}
