class FlagOption {}
class SingleValueOption {
  constructor (definition) {
    this.definition = definition
  }

  set value (val) {
    this._val = this.definition.type(val)
  }
  get value () {
    return this._val
  }
}
class MultipleValueOption {}



class Output extends Map {
  constructor (definitions) {
    this.definitions = definitions
  }

  set (key, value) {
    const def = this.definitions.get(key)

  }
}

const optionDefinitions = [
  { name: 'one' }
]
const output = new Output(optionDefinitions)
output.set('one', 'something')
console.log(output)
output.set('one', 'something2')
console.log(output)
