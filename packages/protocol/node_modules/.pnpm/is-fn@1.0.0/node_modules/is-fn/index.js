'use strict';
var toString = Object.prototype.toString;

module.exports = function (x) {
	return toString.call(x) === '[object Function]';
};
