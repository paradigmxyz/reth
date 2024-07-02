'use strict';

const withIs = require('../../..');

class Animal {
    constructor(type) {
        this.type = type;
    }

    getType() {
        return this.type;
    }
}

module.exports = withIs(Animal, {
    className: 'Animal',
    symbolName: '@org/package/Animal',
});
module.exports.WrappedClass = Animal;
