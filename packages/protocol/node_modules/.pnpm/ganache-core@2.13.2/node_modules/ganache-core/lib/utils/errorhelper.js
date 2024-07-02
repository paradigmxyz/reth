class LevelUpOutOfRangeError extends Error {
  constructor(type, index, len) {
    const message = "LevelUpArrayAdapter named '" + type + "' index out of range: index " + index + "; length: " + len;
    super(message);
    this.name = `${this.constructor.name}:${type}`;
    this.type = type;
  }
}

class BlockOutOfRangeError extends LevelUpOutOfRangeError {
  constructor(index, len) {
    super("blocks", index, len);
  }
}

module.exports = {
  LevelUpOutOfRangeError,
  BlockOutOfRangeError
};
