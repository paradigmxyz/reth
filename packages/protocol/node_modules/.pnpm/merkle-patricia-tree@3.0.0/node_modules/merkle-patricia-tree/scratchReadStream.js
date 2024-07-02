"use strict";

function _typeof(obj) { if (typeof Symbol === "function" && typeof Symbol.iterator === "symbol") { _typeof = function _typeof(obj) { return typeof obj; }; } else { _typeof = function _typeof(obj) { return obj && typeof Symbol === "function" && obj.constructor === Symbol && obj !== Symbol.prototype ? "symbol" : typeof obj; }; } return _typeof(obj); }

function _classCallCheck(instance, Constructor) { if (!(instance instanceof Constructor)) { throw new TypeError("Cannot call a class as a function"); } }

function _defineProperties(target, props) { for (var i = 0; i < props.length; i++) { var descriptor = props[i]; descriptor.enumerable = descriptor.enumerable || false; descriptor.configurable = true; if ("value" in descriptor) descriptor.writable = true; Object.defineProperty(target, descriptor.key, descriptor); } }

function _createClass(Constructor, protoProps, staticProps) { if (protoProps) _defineProperties(Constructor.prototype, protoProps); if (staticProps) _defineProperties(Constructor, staticProps); return Constructor; }

function _possibleConstructorReturn(self, call) { if (call && (_typeof(call) === "object" || typeof call === "function")) { return call; } return _assertThisInitialized(self); }

function _assertThisInitialized(self) { if (self === void 0) { throw new ReferenceError("this hasn't been initialised - super() hasn't been called"); } return self; }

function _getPrototypeOf(o) { _getPrototypeOf = Object.setPrototypeOf ? Object.getPrototypeOf : function _getPrototypeOf(o) { return o.__proto__ || Object.getPrototypeOf(o); }; return _getPrototypeOf(o); }

function _inherits(subClass, superClass) { if (typeof superClass !== "function" && superClass !== null) { throw new TypeError("Super expression must either be null or a function"); } subClass.prototype = Object.create(superClass && superClass.prototype, { constructor: { value: subClass, writable: true, configurable: true } }); if (superClass) _setPrototypeOf(subClass, superClass); }

function _setPrototypeOf(o, p) { _setPrototypeOf = Object.setPrototypeOf || function _setPrototypeOf(o, p) { o.__proto__ = p; return o; }; return _setPrototypeOf(o, p); }

var _require = require('readable-stream'),
    Readable = _require.Readable; // ScratchReadStream
// this is used to minimally dump the scratch into the db


module.exports =
/*#__PURE__*/
function (_Readable) {
  _inherits(ScratchReadStream, _Readable);

  function ScratchReadStream(trie) {
    var _this;

    _classCallCheck(this, ScratchReadStream);

    _this = _possibleConstructorReturn(this, _getPrototypeOf(ScratchReadStream).call(this, {
      objectMode: true
    }));
    _this.trie = trie;
    _this.next = null;
    return _this;
  }

  _createClass(ScratchReadStream, [{
    key: "_read",
    value: function _read() {
      var _this2 = this;

      if (!this._started) {
        this._started = true;

        this.trie._findDbNodes(function (nodeRef, node, key, next) {
          _this2.push({
            key: nodeRef,
            value: node.serialize()
          });

          next();
        }, function () {
          // close stream
          _this2.push(null);
        });
      }
    }
  }]);

  return ScratchReadStream;
}(Readable);