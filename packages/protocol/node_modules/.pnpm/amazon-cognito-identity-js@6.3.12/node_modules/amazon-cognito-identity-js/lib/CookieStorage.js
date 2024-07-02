"use strict";

exports.__esModule = true;
exports["default"] = void 0;
var Cookies = _interopRequireWildcard(require("js-cookie"));
function _getRequireWildcardCache(e) { if ("function" != typeof WeakMap) return null; var r = new WeakMap(), t = new WeakMap(); return (_getRequireWildcardCache = function _getRequireWildcardCache(e) { return e ? t : r; })(e); }
function _interopRequireWildcard(e, r) { if (!r && e && e.__esModule) return e; if (null === e || "object" != typeof e && "function" != typeof e) return { "default": e }; var t = _getRequireWildcardCache(r); if (t && t.has(e)) return t.get(e); var n = { __proto__: null }, a = Object.defineProperty && Object.getOwnPropertyDescriptor; for (var u in e) if ("default" !== u && Object.prototype.hasOwnProperty.call(e, u)) { var i = a ? Object.getOwnPropertyDescriptor(e, u) : null; i && (i.get || i.set) ? Object.defineProperty(n, u, i) : n[u] = e[u]; } return n["default"] = e, t && t.set(e, n), n; }
/** @class */
var CookieStorage = exports["default"] = /*#__PURE__*/function () {
  /**
   * Constructs a new CookieStorage object
   * @param {object} data Creation options.
   * @param {string} data.domain Cookies domain (default: domain of the page
   * 				where the cookie was created, excluding subdomains)
   * @param {string} data.path Cookies path (default: '/')
   * @param {integer} data.expires Cookie expiration (in days, default: 365)
   * @param {boolean} data.secure Cookie secure flag (default: true)
   * @param {string} data.sameSite Cookie request behavior (default: null)
   */
  function CookieStorage(data) {
    if (data === void 0) {
      data = {};
    }
    if (data.domain) {
      this.domain = data.domain;
    }
    if (data.path) {
      this.path = data.path;
    } else {
      this.path = '/';
    }
    if (Object.prototype.hasOwnProperty.call(data, 'expires')) {
      this.expires = data.expires;
    } else {
      this.expires = 365;
    }
    if (Object.prototype.hasOwnProperty.call(data, 'secure')) {
      this.secure = data.secure;
    } else {
      this.secure = true;
    }
    if (Object.prototype.hasOwnProperty.call(data, 'sameSite')) {
      if (!['strict', 'lax', 'none'].includes(data.sameSite)) {
        throw new Error('The sameSite value of cookieStorage must be "lax", "strict" or "none".');
      }
      if (data.sameSite === 'none' && !this.secure) {
        throw new Error('sameSite = None requires the Secure attribute in latest browser versions.');
      }
      this.sameSite = data.sameSite;
    } else {
      this.sameSite = null;
    }
  }

  /**
   * This is used to set a specific item in storage
   * @param {string} key - the key for the item
   * @param {object} value - the value
   * @returns {string} value that was set
   */
  var _proto = CookieStorage.prototype;
  _proto.setItem = function setItem(key, value) {
    var options = {
      path: this.path,
      expires: this.expires,
      domain: this.domain,
      secure: this.secure
    };
    if (this.sameSite) {
      options.sameSite = this.sameSite;
    }
    Cookies.set(key, value, options);
    return Cookies.get(key);
  }

  /**
   * This is used to get a specific key from storage
   * @param {string} key - the key for the item
   * This is used to clear the storage
   * @returns {string} the data item
   */;
  _proto.getItem = function getItem(key) {
    return Cookies.get(key);
  }

  /**
   * This is used to remove an item from storage
   * @param {string} key - the key being set
   * @returns {string} value - value that was deleted
   */;
  _proto.removeItem = function removeItem(key) {
    var options = {
      path: this.path,
      expires: this.expires,
      domain: this.domain,
      secure: this.secure
    };
    if (this.sameSite) {
      options.sameSite = this.sameSite;
    }
    return Cookies.remove(key, options);
  }

  /**
   * This is used to clear the storage of optional
   * items that were previously set
   * @returns {} an empty object
   */;
  _proto.clear = function clear() {
    var cookies = Cookies.get();
    var numKeys = Object.keys(cookies).length;
    for (var index = 0; index < numKeys; ++index) {
      this.removeItem(Object.keys(cookies)[index]);
    }
    return {};
  };
  return CookieStorage;
}();