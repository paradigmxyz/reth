interface Task {
  priority: number
  fn: Function
}

export class PrioritizedTaskExecutor {
  /** The maximum size of the pool */
  private maxPoolSize: number
  /** The current size of the pool */
  private currentPoolSize: number
  /** The task queue */
  private queue: Task[]

  /**
   * Executes tasks up to maxPoolSize at a time, other items are put in a priority queue.
   * @class PrioritizedTaskExecutor
   * @private
   * @param maxPoolSize The maximum size of the pool
   */
  constructor(maxPoolSize: number) {
    this.maxPoolSize = maxPoolSize
    this.currentPoolSize = 0
    this.queue = []
  }

  /**
   * Executes the task or queues it if no spots are available.
   * When a task is added, check if there are spots left in the pool.
   * If a spot is available, claim that spot and give back the spot once the asynchronous task has been resolved.
   * When no spots are available, add the task to the task queue. The task will be executed at some point when another task has been resolved.
   * @private
   * @param priority The priority of the task
   * @param fn The function that accepts the callback, which must be called upon the task completion.
   */
  executeOrQueue(priority: number, fn: Function) {
    if (this.currentPoolSize < this.maxPoolSize) {
      this.currentPoolSize++
      fn(() => {
        this.currentPoolSize--
        if (this.queue.length > 0) {
          this.queue.sort((a, b) => b.priority - a.priority)
          const item = this.queue.shift()
          this.executeOrQueue(item!.priority, item!.fn)
        }
      })
    } else {
      this.queue.push({ priority, fn })
    }
  }

  /**
   * Checks if the taskExecutor is finished.
   * @private
   * @returns Returns `true` if the taskExecutor is finished, otherwise returns `false`.
   */
  finished(): boolean {
    return this.currentPoolSize === 0
  }
}
