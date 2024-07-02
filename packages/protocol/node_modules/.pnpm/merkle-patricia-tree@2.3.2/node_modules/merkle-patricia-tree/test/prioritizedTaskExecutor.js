const PrioritizedTaskExecutor = require('../prioritizedTaskExecutor.js')
const tape = require('tape')
const taskExecutor = new PrioritizedTaskExecutor(2)

tape('prioritized task executor test', function (t) {
  var tasks = [1, 2, 3, 4]
  var callbacks = []
  var executionOrder = []
  tasks.forEach(function (task) {
    taskExecutor.execute(task, function (cb) {
      executionOrder.push(task)
      callbacks.push(cb)
    })
  })

  callbacks.forEach(function (callback) {
    callback()
  })

  var expectedExecutionOrder = [1, 2, 4, 3]
  t.deepEqual(executionOrder, expectedExecutionOrder)
  t.end()
})
