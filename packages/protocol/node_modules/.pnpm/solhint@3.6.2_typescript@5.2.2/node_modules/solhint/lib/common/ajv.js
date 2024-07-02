const Ajv = require('ajv')
const metaSchema = require('ajv/lib/refs/json-schema-draft-04.json')

const ajv = new Ajv({
  meta: false,
  validateSchema: false,
  missingRefs: 'ignore',
  verbose: true,
  schemaId: 'auto',
})

ajv.addMetaSchema(metaSchema)
ajv._opts.defaultMeta = metaSchema.id

module.exports = ajv
