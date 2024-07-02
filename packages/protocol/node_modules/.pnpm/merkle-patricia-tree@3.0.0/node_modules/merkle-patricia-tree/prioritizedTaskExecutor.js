"use strict";

function _classCallCheck(instance, Constructor) { if (!(instance instanceof Constructor)) { throw new TypeError("Cannot call a class as a function"); } }

function _defineProperties(target, props) { for (var i = 0; i < props.length; i++) { var descriptor = props[i]; descriptor.enumerable = descriptor.enumerable || false; descriptor.configurable = true; if ("value" in descriptor) descriptor.writable = true; Object.defineProperty(target, descriptor.key, descriptor); } }

function _createClass(Constructor, protoProps, staticProps) { if (protoProps) _defineProperties(Constructor.prototype, protoProps); if (staticProps) _defineProperties(Constructor, staticProps); return Constructor; }

module.exports =
/*#__PURE__*/
function () {
  /**
   * Executes tasks up to maxPoolSize at a time, other items are put in a priority queue.
   * @class PrioritizedTaskExecutor
   * @private
   * @param {Number} maxPoolSize The maximum size of the pool
   * @prop {Number} maxPoolSize The maximum size of the pool
   * @prop {Number} currentPoolSize The current size of the pool
   * @prop {Array} queue The task queue
   */
  function PrioritizedTaskExecutor(maxPoolSize) {
    _classCallCheck(this, PrioritizedTaskExecutor);

    this.maxPoolSize = maxPoolSize;
    this.currentPoolSize = 0;
    this.queue = [];
  }
  /**
   * Executes the task.
   * @private
   * @param {Number} priority The priority of the task
   * @param {Function} task The function that accepts the callback, which must be called upon the task completion.
   */


  _createClass(PrioritizedTaskExecutor, [{
    key: "execute",
    value: function execute(priority, task) {
      var _this = this;

      if (this.currentPoolSize < this.maxPoolSize) {
        this.currentPoolSize++;
        task(function () {
          _this.currentPoolSize--;

          if (_this.queue.length > 0) {
            _this.queue.sort(function (a, b) {
              return b.priority - a.priority;
            });

            var item = _this.queue.shift();

            _this.execute(item.priority, item.task);
          }
        });
      } else {
        this.queue.push({
          priority: priority,
          task: task
        });
      }
    }
  }]);

  return PrioritizedTaskExecutor;
}();