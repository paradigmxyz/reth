'use strict';

const withIs = require('../../..');

function Animal(type) {
    this.type = type;
}

Animal.prototype.getType = function () {
    return this.type;
};

module.exports = withIs.proto(Animal, {
    className: 'Animal',
    symbolName: '@org/package/Animal',
});
module.exports.WrappedClass = Animal;
