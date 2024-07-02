class SecurityErrorListener {
  constructor(checkers) {
    this.listenersMap = {}

    checkers.forEach((i) => this.addChecker(i))
  }

  addChecker(newChecker) {
    const listenerMethods = Object.getOwnPropertyNames(newChecker.constructor.prototype)

    const usedListenerMethods = listenerMethods.filter((i) => /^[A-Z]/.test(i))

    usedListenerMethods.forEach((methodName) => this.addNewListener(methodName, newChecker))

    usedListenerMethods.forEach((methodName) => {
      this[methodName] = this.notifyListenersOn(methodName)
    })
  }

  listenersFor(name) {
    return this.listenersMap[name] || []
  }

  addNewListener(methodName, checker) {
    const method = checker[methodName].bind(checker)
    const listeners = this.listenersFor(methodName)
    this.listenersMap[methodName] = listeners.concat(method)
  }

  notifyListenersOn(methodName) {
    return (ctx) => this.listenersFor(methodName).forEach((fn) => quite(fn)(ctx))
  }
}

function quite(fn) {
  return (...args) => {
    try {
      if (fn) {
        fn.call(fn, ...args)
      }
    } catch (err) {
      // console.log(err);
    }
  }
}

module.exports = SecurityErrorListener
