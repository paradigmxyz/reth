const Backoff = require('backoff')
const EventEmitter = require('events')
const inherits = require('util').inherits
const WebSocket = global.WebSocket || require('ws')
const Subprovider = require('./subprovider')
const createPayload = require('../util/create-payload')

class WebsocketSubprovider
 extends Subprovider {
  constructor({ rpcUrl, debug, origin }) {
    super()

    // inherit from EventEmitter
    EventEmitter.call(this)

    Object.defineProperties(this, {
      _backoff: {
        value: Backoff.exponential({
          randomisationFactor: 0.2,
          maxDelay: 5000
        })
      },
      _connectTime: {
        value: null,
        writable: true
      },
      _log: {
        value: debug
          ? (...args) => console.info.apply(console, ['[WSProvider]', ...args])
          : () => { }
      },
      _origin: {
        value: origin
      },
      _pendingRequests: {
        value: new Map()
      },
      _socket: {
        value: null,
        writable: true
      },
      _unhandledRequests: {
        value: []
      },
      _url: {
        value: rpcUrl
      }
    })

    this._handleSocketClose = this._handleSocketClose.bind(this)
    this._handleSocketMessage = this._handleSocketMessage.bind(this)
    this._handleSocketOpen = this._handleSocketOpen.bind(this)

    // Called when a backoff timeout has finished. Time to try reconnecting.
    this._backoff.on('ready', () => {
      this._openSocket()
    })

    this._openSocket()
  }

  handleRequest(payload, next, end) {
    if (!this._socket || this._socket.readyState !== WebSocket.OPEN) {
      this._unhandledRequests.push(Array.from(arguments))
      this._log('Socket not open. Request queued.')
      return
    }

    this._pendingRequests.set(payload.id, [payload, end])

    const newPayload = createPayload(payload)
    delete newPayload.origin

    this._socket.send(JSON.stringify(newPayload))
    this._log(`Sent: ${newPayload.method} #${newPayload.id}`)
  }

  _handleSocketClose({ reason, code }) {
    this._log(`Socket closed, code ${code} (${reason || 'no reason'})`)
    // If the socket has been open for longer than 5 seconds, reset the backoff
    if (this._connectTime && Date.now() - this._connectTime > 5000) {
      this._backoff.reset()
    }

    this._socket.removeEventListener('close', this._handleSocketClose)
    this._socket.removeEventListener('message', this._handleSocketMessage)
    this._socket.removeEventListener('open', this._handleSocketOpen)

    this._socket = null
    this._backoff.backoff()
  }

  _handleSocketMessage(message) {
    let payload

    try {
      payload = JSON.parse(message.data)
    } catch (e) {
      this._log('Received a message that is not valid JSON:', payload)
      return
    }

    // check if server-sent notification
    if (payload.id === undefined) {
      return this.emit('data', null, payload)
    }

    // ignore if missing
    if (!this._pendingRequests.has(payload.id)) {
      return
    }

    // retrieve payload + arguments
    const [originalReq, end] = this._pendingRequests.get(payload.id)
    this._pendingRequests.delete(payload.id)

    this._log(`Received: ${originalReq.method} #${payload.id}`)

    // forward response
    if (payload.error) {
      return end(new Error(payload.error.message))
    }
    end(null, payload.result)
  }

  _handleSocketOpen() {
    this._log('Socket open.')
    this._connectTime = Date.now()

    // Any pending requests need to be resent because our session was lost
    // and will not get responses for them in our new session.
    this._pendingRequests.forEach(([payload, end]) => {
      this._unhandledRequests.push([payload, null, end])
    })
    this._pendingRequests.clear()

    const unhandledRequests = this._unhandledRequests.splice(0, this._unhandledRequests.length)
    unhandledRequests.forEach(request => {
      this.handleRequest.apply(this, request)
    })
  }

  _openSocket() {
    this._log('Opening socket...')
    this._socket = new WebSocket(this._url, [], this._origin ? {headers:{origin: this._origin}} : {})
    this._socket.addEventListener('close', this._handleSocketClose)
    this._socket.addEventListener('message', this._handleSocketMessage)
    this._socket.addEventListener('open', this._handleSocketOpen)
  }
}

// multiple inheritance
Object.assign(WebsocketSubprovider.prototype, EventEmitter.prototype)

module.exports = WebsocketSubprovider
