'use strict';

const withIs = require('../../..');

function Plant(type) {
    this.type = type;
}

Plant.prototype.getType = function () {
    return this.type;
};

module.exports = withIs.proto(Plant, {
    className: 'Plant',
    symbolName: '@org/package/Plant',
});
module.exports.WrappedClass = Plant;
