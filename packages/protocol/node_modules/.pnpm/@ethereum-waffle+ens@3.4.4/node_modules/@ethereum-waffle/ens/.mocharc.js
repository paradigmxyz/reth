process.env.NODE_ENV = 'test'
module.exports = {
  require: 'ts-node/register/transpile-only',
  timeout: 50000,
  spec: 'test/**/*.{js,ts}'
}
