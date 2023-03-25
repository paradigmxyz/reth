/// Nibbles: A nibble is a 4-bit data unit, which can represent one of 16 possible values (0-9,
/// A-F). In the context of Ethereum's MPT, nibbles are used to break down the keys in the trie.
/// Each key in the trie is a 32-byte (256-bit) hash, which can be represented as a sequence of 64
/// hexadecimal characters. Since each hexadecimal character is a nibble, keys in Ethereum's MPT can
/// be represented as a sequence of 64 nibbles.
pub mod nibbles;


pub mod node;