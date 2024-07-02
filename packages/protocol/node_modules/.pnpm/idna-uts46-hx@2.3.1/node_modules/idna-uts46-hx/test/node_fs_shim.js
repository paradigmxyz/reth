"use strict";

var fs = {};
fs.readFile = function(filename, options, callback) {
  // Note: options is the optional element here, not callback.
  if (arguments.length < 3) {
    callback = options;
    options = {};
  }
  if (filename[0] === '/')
    throw new Error("Don't use absolute paths!");
  var xhr = new XMLHttpRequest();
  xhr.open("GET", filename);
  if (options && options.encoding)
    xhr.overrideMimeType('text/plain; charset=' + options.encoding);
  else {
    xhr.overrideMimeType('text/plain');
    xhr.responseType = "arraybuffer";
  }
  xhr.onload = function() {
    if (xhr.status > 400)
      callback(new Error(xhr.statusText));
    else
      callback(undefined, xhr.response);
  };
  xhr.onerror = function(e) {
    callback(e);
  };
  xhr.send();
};
