const { validSeverityMap, invalidSeverityMap } = require('../config/config-validator')

const formatEnum = (options) => options.map((op) => JSON.stringify(op)).join(', ')
const ruleSeverityEnum = formatEnum([...validSeverityMap, ...invalidSeverityMap])

module.exports = {
  ruleSeverityEnum,
  severityDescription: `Rule severity. Must be one of ${ruleSeverityEnum}.`,
  formatEnum,
}
