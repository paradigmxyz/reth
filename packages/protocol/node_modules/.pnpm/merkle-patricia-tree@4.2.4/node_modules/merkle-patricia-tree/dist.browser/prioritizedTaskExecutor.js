"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.PrioritizedTaskExecutor = void 0;
var PrioritizedTaskExecutor = /** @class */ (function () {
    /**
     * Executes tasks up to maxPoolSize at a time, other items are put in a priority queue.
     * @class PrioritizedTaskExecutor
     * @private
     * @param maxPoolSize The maximum size of the pool
     */
    function PrioritizedTaskExecutor(maxPoolSize) {
        this.maxPoolSize = maxPoolSize;
        this.currentPoolSize = 0;
        this.queue = [];
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
    PrioritizedTaskExecutor.prototype.executeOrQueue = function (priority, fn) {
        var _this = this;
        if (this.currentPoolSize < this.maxPoolSize) {
            this.currentPoolSize++;
            fn(function () {
                _this.currentPoolSize--;
                if (_this.queue.length > 0) {
                    _this.queue.sort(function (a, b) { return b.priority - a.priority; });
                    var item = _this.queue.shift();
                    _this.executeOrQueue(item.priority, item.fn);
                }
            });
        }
        else {
            this.queue.push({ priority: priority, fn: fn });
        }
    };
    /**
     * Checks if the taskExecutor is finished.
     * @private
     * @returns Returns `true` if the taskExecutor is finished, otherwise returns `false`.
     */
    PrioritizedTaskExecutor.prototype.finished = function () {
        return this.currentPoolSize === 0;
    };
    return PrioritizedTaskExecutor;
}());
exports.PrioritizedTaskExecutor = PrioritizedTaskExecutor;
//# sourceMappingURL=prioritizedTaskExecutor.js.map