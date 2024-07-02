'use strict';

var jsonStringify = (typeof JSON !== 'undefined' ? JSON : require('jsonify')).stringify;

var isArray = require('isarray');
var objectKeys = require('object-keys');
var callBind = require('call-bind');
var callBound = require('call-bind/callBound');

var $join = callBound('Array.prototype.join');
var $push = callBound('Array.prototype.push');

var strRepeat = function repeat(n, char) {
	var str = '';
	for (var i = 0; i < n; i += 1) {
		str += char;
	}
	return str;
};

var defaultReplacer = function (parent, key, value) { return value; };

module.exports = function stableStringify(obj) {
	var opts = arguments.length > 1 ? arguments[1] : void undefined;
	var space = (opts && opts.space) || '';
	if (typeof space === 'number') { space = strRepeat(space, ' '); }
	var cycles = !!opts && typeof opts.cycles === 'boolean' && opts.cycles;
	var replacer = opts && opts.replacer ? callBind(opts.replacer) : defaultReplacer;

	var cmpOpt = typeof opts === 'function' ? opts : opts && opts.cmp;
	var cmp = cmpOpt && function (node) {
		var get = cmpOpt.length > 2 && function get(k) { return node[k]; };
		return function (a, b) {
			return cmpOpt(
				{ key: a, value: node[a] },
				{ key: b, value: node[b] },
				get ? { __proto__: null, get: get } : void undefined
			);
		};
	};

	var seen = [];
	return (function stringify(parent, key, node, level) {
		var indent = space ? '\n' + strRepeat(level, space) : '';
		var colonSeparator = space ? ': ' : ':';

		if (node && node.toJSON && typeof node.toJSON === 'function') {
			node = node.toJSON();
		}

		node = replacer(parent, key, node);

		if (node === undefined) {
			return;
		}
		if (typeof node !== 'object' || node === null) {
			return jsonStringify(node);
		}
		if (isArray(node)) {
			var out = [];
			for (var i = 0; i < node.length; i++) {
				var item = stringify(node, i, node[i], level + 1) || jsonStringify(null);
				$push(out, indent + space + item);
			}
			return '[' + $join(out, ',') + indent + ']';
		}

		if (seen.indexOf(node) !== -1) {
			if (cycles) { return jsonStringify('__cycle__'); }
			throw new TypeError('Converting circular structure to JSON');
		} else { $push(seen, node); }

		var keys = objectKeys(node).sort(cmp && cmp(node));
		var out = [];
		for (var i = 0; i < keys.length; i++) {
			var key = keys[i];
			var value = stringify(node, key, node[key], level + 1);

			if (!value) { continue; }

			var keyValue = jsonStringify(key)
				+ colonSeparator
				+ value;

			$push(out, indent + space + keyValue);
		}
		seen.splice(seen.indexOf(node), 1);
		return '{' + $join(out, ',') + indent + '}';

	}({ '': obj }, '', obj, 0));
};
