module.exports = PrioritizedTaskExecutor

/**
 * Executes tasks up to maxPoolSize at a time, other items are put in a priority queue.
 * @class PrioritizedTaskExecutor
 * @param {Number} maxPoolSize The maximum size of the pool
 * @prop {Number} maxPoolSize The maximum size of the pool
 * @prop {Number} currentPoolSize The current size of the pool
 * @prop {Array} queue The task queue
 */
function PrioritizedTaskExecutor (maxPoolSize) {
  this.maxPoolSize = maxPoolSize
  this.currentPoolSize = 0
  this.queue = []
}

/**
 * Executes the task.
 * @param {Number} priority The priority of the task
 * @param {Function} task The function that accepts the callback, which must be called upon the task completion.
 */
PrioritizedTaskExecutor.prototype.execute = function (priority, task) {
  var self = this

  if (self.currentPoolSize < self.maxPoolSize) {
    self.currentPoolSize++
    task(function () {
      self.currentPoolSize--
      if (self.queue.length > 0) {
        self.queue.sort(function (a, b) {
          return b.priority - a.priority
        })
        var item = self.queue.shift()
        self.execute(item.priority, item.task)
      }
    })
  } else {
    self.queue.push({
      priority: priority,
      task: task
    })
  }
}
