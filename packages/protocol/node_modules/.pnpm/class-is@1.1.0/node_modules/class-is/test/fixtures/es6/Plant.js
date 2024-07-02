'use strict';

const withIs = require('../../..');

class Plant {
    constructor(type) {
        this.type = type;
    }

    getType() {
        return this.type;
    }
}

module.exports = withIs(Plant, {
    className: 'Plant',
    symbolName: '@org/package/Plant',
});
module.exports.WrappedClass = Plant;
