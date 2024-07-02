/*!
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
/** @class */
var CognitoUserAttribute = /*#__PURE__*/function () {
  /**
   * Constructs a new CognitoUserAttribute object
   * @param {string=} Name The record's name
   * @param {string=} Value The record's value
   */
  function CognitoUserAttribute(_temp) {
    var _ref = _temp === void 0 ? {} : _temp,
      Name = _ref.Name,
      Value = _ref.Value;
    this.Name = Name || '';
    this.Value = Value || '';
  }

  /**
   * @returns {string} the record's value.
   */
  var _proto = CognitoUserAttribute.prototype;
  _proto.getValue = function getValue() {
    return this.Value;
  }

  /**
   * Sets the record's value.
   * @param {string} value The new value.
   * @returns {CognitoUserAttribute} The record for method chaining.
   */;
  _proto.setValue = function setValue(value) {
    this.Value = value;
    return this;
  }

  /**
   * @returns {string} the record's name.
   */;
  _proto.getName = function getName() {
    return this.Name;
  }

  /**
   * Sets the record's name
   * @param {string} name The new name.
   * @returns {CognitoUserAttribute} The record for method chaining.
   */;
  _proto.setName = function setName(name) {
    this.Name = name;
    return this;
  }

  /**
   * @returns {string} a string representation of the record.
   */;
  _proto.toString = function toString() {
    return JSON.stringify(this);
  }

  /**
   * @returns {object} a flat object representing the record.
   */;
  _proto.toJSON = function toJSON() {
    return {
      Name: this.Name,
      Value: this.Value
    };
  };
  return CognitoUserAttribute;
}();
export { CognitoUserAttribute as default };