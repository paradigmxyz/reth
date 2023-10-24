use reth_primitives::{hex, Bytecode, Bytes};
use reth_revm::{
    interpreter::{opcode, OpCode},
    tracing::OpcodeCountInspector,
};
use std::collections::HashMap;

fn main() -> eyre::Result<()> {
    let test_bytecode = get_test_bytecode_occurrencies_1();
    let bytecode = Bytecode::new_raw(hex::decode(test_bytecode).unwrap().into());
    let bytes = filter_bytecode_bytes(bytecode.bytes());

    let mut counter = OpCodeCounter::new();
    counter.count_sequences(&bytes);

    counter.print_counts();
    Ok(())
}

struct OpCodeCounter {
    opcodes: HashMap<u8, usize>,
    tuple_opcodes: HashMap<[u8; 2], usize>,
    triplets_opcodes: HashMap<[u8; 3], usize>,
    quadruplets_opcodes: HashMap<[u8; 4], usize>,
}

impl OpCodeCounter {
    fn new() -> Self {
        Self {
            opcodes: HashMap::new(),
            tuple_opcodes: HashMap::new(),
            triplets_opcodes: HashMap::new(),
            quadruplets_opcodes: HashMap::new(),
        }
    }

    fn count_sequences(&mut self, bytes: &Bytes) {
        for (i, opcode) in bytes.iter().enumerate() {
            let tuple = bytes.get(i..=i + 1);
            let triple = bytes.get(i..=i + 2);
            let quadruple = bytes.get(i..=i + 3);

            *self.opcodes.entry(*opcode).or_default() += 1;
            if let Some(t) = tuple {
                *self.tuple_opcodes.entry([t[0], t[1]]).or_default() += 1;
            }
            if let Some(t) = triple {
                *self.triplets_opcodes.entry([t[0], t[1], t[2]]).or_default() += 1;
            }
            if let Some(t) = quadruple {
                *self.quadruplets_opcodes.entry([t[0], t[1], t[2], t[3]]).or_default() += 1;
            }
        }
    }

    fn print_counts(&self) {
        println!("Single opcodes:");
        for (opcode, occurencies) in &self.opcodes {
            match OpCode::new(*opcode) {
                Some(op) => println!("{}: {}", op, occurencies),
                None => println!("{}: {}", opcode, occurencies),
            };
        }
        println!("----------------------------------------------");

        let mut tuple_vec: Vec<(OpCode, OpCode, usize)> = self
            .tuple_opcodes
            .iter()
            .map(|(k, v)| (OpCode::new(k[0]).unwrap(), OpCode::new(k[1]).unwrap(), *v))
            .collect();

        tuple_vec.sort_by(|e1, e2| e1.2.cmp(&e2.2));
        tuple_vec.reverse();

        println!("Tuple opcodes:");
        for el in &tuple_vec {
            println!("{} {}: {}", el.0, el.1, el.2);
        }
        // for (tuple_opcode, occurencies) in &self.tuple_opcodes {
        //     let op1 = OpCode::new(tuple_opcode[0])
        //         .unwrap_or_else(|| panic!("Invalid opcode: {}", tuple_opcode[0]));
        //     let op2 = OpCode::new(tuple_opcode[1])
        //         .unwrap_or_else(|| panic!("Invalid opcode: {}", tuple_opcode[1]));
        //     println!("{} {}: {}", op1, op2, occurencies);
        // }
        println!("----------------------------------------------");

        println!("Triplet opcodes:");
        for (triplet_opcodes, occurencies) in &self.triplets_opcodes {
            let op1 = OpCode::new(triplet_opcodes[0])
                .unwrap_or_else(|| panic!("Invalid opcode: {}", triplet_opcodes[0]));
            let op2 = OpCode::new(triplet_opcodes[1])
                .unwrap_or_else(|| panic!("Invalid opcode: {}", triplet_opcodes[1]));
            let op3 = OpCode::new(triplet_opcodes[2])
                .unwrap_or_else(|| panic!("Invalid opcode: {}", triplet_opcodes[2]));
            println!("{} {} {}: {}", op1, op2, op3, occurencies);
        }
        println!("----------------------------------------------");

        println!("Quadruplet opcodes:");
        for (quadruplet_opcodes, occurencies) in &self.quadruplets_opcodes {
            let op1 = OpCode::new(quadruplet_opcodes[0])
                .unwrap_or_else(|| panic!("Invalid opcode: {}", quadruplet_opcodes[0]));
            let op2 = OpCode::new(quadruplet_opcodes[1])
                .unwrap_or_else(|| panic!("Invalid opcode: {}", quadruplet_opcodes[1]));
            let op3 = OpCode::new(quadruplet_opcodes[2])
                .unwrap_or_else(|| panic!("Invalid opcode: {}", quadruplet_opcodes[2]));
            let op4 = OpCode::new(quadruplet_opcodes[3])
                .unwrap_or_else(|| panic!("Invalid opcode: {}", quadruplet_opcodes[3]));
            println!("{} {} {} {}: {}", op1, op2, op3, op4, occurencies);
        }
        println!("----------------------------------------------");
    }
}

/// Takes bytecode bytes and returns filtered bytes without `PUSH` data
pub fn filter_bytecode_bytes(bytes: &Bytes) -> Bytes {
    let mut push_data_to_skip = 0_usize;

    let iter = bytes.iter().filter(|op| {
        if push_data_to_skip > 0 {
            push_data_to_skip -= 1;
            return false;
        };
        if (opcode::PUSH1..=opcode::PUSH32).contains(op) {
            push_data_to_skip = (**op - opcode::PUSH1 + 1) as usize;
        };
        true
    });

    Bytes::from_iter(iter)
}

fn get_test_bytecode_occurrencies_1() -> String {
    let test_bytecode = "00"; // STOP 13
    let test_bytecode = format!("{test_bytecode}01"); // ADD
    let test_bytecode = format!("{test_bytecode}02"); // MUL
    let test_bytecode = format!("{test_bytecode}03"); // SUB
    let test_bytecode = format!("{test_bytecode}05"); // SDIV
    let test_bytecode = format!("{test_bytecode}04"); // DIV
    let test_bytecode = format!("{test_bytecode}05"); // SDIV
    let test_bytecode = format!("{test_bytecode}06"); // MOD
    let test_bytecode = format!("{test_bytecode}09"); // MULMOD
    let test_bytecode = format!("{test_bytecode}07"); // SMOD
    let test_bytecode = format!("{test_bytecode}08"); // ADDMOD
    let test_bytecode = format!("{test_bytecode}09"); // MULMOD
    let test_bytecode = format!("{test_bytecode}10"); // LT
    let test_bytecode = format!("{test_bytecode}04"); // DIV
    let test_bytecode = format!("{test_bytecode}04"); // DIV
    let test_bytecode = format!("{test_bytecode}04"); // DIV
    let test_bytecode = format!("{test_bytecode}04"); // DIV
    let test_bytecode = format!("{test_bytecode}04"); // DIV
    let test_bytecode = format!("{test_bytecode}0A"); // EXP
    let test_bytecode = format!("{test_bytecode}0B"); // SIGNEXTEND

    test_bytecode
}

fn get_test_bytecode_occurrencies_2() -> String {
    let test_bytecode = "00"; // STOP 13
    let test_bytecode = format!("{test_bytecode}01"); // ADD
    let test_bytecode = format!("{test_bytecode}02"); // MUL
    let test_bytecode = format!("{test_bytecode}10"); // LT
    let test_bytecode = format!("{test_bytecode}02"); // MUL
    let test_bytecode = format!("{test_bytecode}01"); // ADD
    let test_bytecode = format!("{test_bytecode}02"); // MUL
    let test_bytecode = format!("{test_bytecode}0A"); // EXP

    test_bytecode
}

fn get_test_bytecode() -> String {
    let test_bytecode = "00"; // STOP 13
    let test_bytecode = format!("{test_bytecode}01"); // ADD
    let test_bytecode = format!("{test_bytecode}02"); // MUL
    let test_bytecode = format!("{test_bytecode}03"); // SUB
    let test_bytecode = format!("{test_bytecode}04"); // DIV
    let test_bytecode = format!("{test_bytecode}05"); // SDIV
    let test_bytecode = format!("{test_bytecode}06"); // MOD
    let test_bytecode = format!("{test_bytecode}07"); // SMOD
    let test_bytecode = format!("{test_bytecode}08"); // ADDMOD
    let test_bytecode = format!("{test_bytecode}09"); // MULMOD
    let test_bytecode = format!("{test_bytecode}0A"); // EXP
    let test_bytecode = format!("{test_bytecode}0B"); // SIGNEXTEND
    let test_bytecode = format!("{test_bytecode}10"); // LT
    let test_bytecode = format!("{test_bytecode}11"); // GT
    let test_bytecode = format!("{test_bytecode}12"); // SLT
    let test_bytecode = format!("{test_bytecode}13"); // SGT
    let test_bytecode = format!("{test_bytecode}14"); // EQ
    let test_bytecode = format!("{test_bytecode}15"); // ISZERO
    let test_bytecode = format!("{test_bytecode}16"); // AND
    let test_bytecode = format!("{test_bytecode}17"); // OR
    let test_bytecode = format!("{test_bytecode}18"); // XOR
    let test_bytecode = format!("{test_bytecode}19"); // NOT
    let test_bytecode = format!("{test_bytecode}1A"); // BYTE
    let test_bytecode = format!("{test_bytecode}1B"); // SHL
    let test_bytecode = format!("{test_bytecode}1C"); // SHR
    let test_bytecode = format!("{test_bytecode}1D"); // SAR
    let test_bytecode = format!("{test_bytecode}20"); // KECCAK256
    let test_bytecode = format!("{test_bytecode}30"); // ADDRESS
    let test_bytecode = format!("{test_bytecode}31"); // BALANCE
    let test_bytecode = format!("{test_bytecode}32"); // ORIGIN
    let test_bytecode = format!("{test_bytecode}33"); // CALLER
    let test_bytecode = format!("{test_bytecode}34"); // CALLVALUE
    let test_bytecode = format!("{test_bytecode}35"); // CALLDATALOAD
    let test_bytecode = format!("{test_bytecode}36"); // CALLDATASIZE
    let test_bytecode = format!("{test_bytecode}37"); // CALLDATACOPY
    let test_bytecode = format!("{test_bytecode}38"); // CODESIZE
    let test_bytecode = format!("{test_bytecode}39"); // CODECOPY
    let test_bytecode = format!("{test_bytecode}3A"); // GASPRICE
    let test_bytecode = format!("{test_bytecode}3B"); // EXTCODESIZE
    let test_bytecode = format!("{test_bytecode}3C"); // EXTCODECOPY
    let test_bytecode = format!("{test_bytecode}3D"); // RETURNDATASIZE
    let test_bytecode = format!("{test_bytecode}3E"); // RETURNDATACOPY
    let test_bytecode = format!("{test_bytecode}3F"); // EXTCODEHASH
    let test_bytecode = format!("{test_bytecode}40"); // BLOCKHASH
    let test_bytecode = format!("{test_bytecode}41"); // COINBASE
    let test_bytecode = format!("{test_bytecode}42"); // TIMESTAMP
    let test_bytecode = format!("{test_bytecode}43"); // NUMBER
    let test_bytecode = format!("{test_bytecode}44"); // DIFFICULTY
    let test_bytecode = format!("{test_bytecode}45"); // GASLIMIT
    let test_bytecode = format!("{test_bytecode}46"); // CHAINID
    let test_bytecode = format!("{test_bytecode}47"); // SELFBALANCE
    let test_bytecode = format!("{test_bytecode}48"); // BASEFEE
    let test_bytecode = format!("{test_bytecode}49"); // BLOBHASH
    let test_bytecode = format!("{test_bytecode}4A"); // BLOBBASEFEE
    let test_bytecode = format!("{test_bytecode}50"); // POP
    let test_bytecode = format!("{test_bytecode}51"); // MLOAD
    let test_bytecode = format!("{test_bytecode}52"); // MSTORE
    let test_bytecode = format!("{test_bytecode}53"); // MSTORE8
    let test_bytecode = format!("{test_bytecode}54"); // SLOAD
    let test_bytecode = format!("{test_bytecode}55"); // SSTORE
    let test_bytecode = format!("{test_bytecode}56"); // JUMP
    let test_bytecode = format!("{test_bytecode}57"); // JUMPI
    let test_bytecode = format!("{test_bytecode}58"); // PC
    let test_bytecode = format!("{test_bytecode}59"); // MSIZE
    let test_bytecode = format!("{test_bytecode}5A"); // GAS
    let test_bytecode = format!("{test_bytecode}5B"); // JUMPDEST
    let test_bytecode = format!("{test_bytecode}5C"); // TLOAD
    let test_bytecode = format!("{test_bytecode}5D"); // TSTORE
    let test_bytecode = format!("{test_bytecode}5E"); // MCOPY
    let test_bytecode = format!("{test_bytecode}5F"); // PUSH0
    let test_bytecode = format!("{test_bytecode}60aa"); // PUSH1
    let test_bytecode = format!("{test_bytecode}61aaaa"); // PUSH2
    let test_bytecode = format!("{test_bytecode}62aaaaaa"); // PUSH3
    let test_bytecode = format!("{test_bytecode}63aaaaaaaa"); // PUSH4
    let test_bytecode = format!("{test_bytecode}64aaaaaaaaaa"); // PUSH5
    let test_bytecode = format!("{test_bytecode}65aaaaaaaaaaaa"); // PUSH6
    let test_bytecode = format!("{test_bytecode}66aaaaaaaaaaaaaa"); // PUSH7
    let test_bytecode = format!("{test_bytecode}67aaaaaaaaaaaaaaaa"); // PUSH8
    let test_bytecode = format!("{test_bytecode}68aaaaaaaaaaaaaaaaaa"); // PUSH9
    let test_bytecode = format!("{test_bytecode}69aaaaaaaaaaaaaaaaaaaa"); // PUSH10
    let test_bytecode = format!("{test_bytecode}6Aaaaaaaaaaaaaaaaaaaaaaa"); // PUSH11
    let test_bytecode = format!("{test_bytecode}6Baaaaaaaaaaaaaaaaaaaaaaaa"); // PUSH12
    let test_bytecode = format!("{test_bytecode}6Caaaaaaaaaaaaaaaaaaaaaaaaaa"); // PUSH13
    let test_bytecode = format!("{test_bytecode}6Daaaaaaaaaaaaaaaaaaaaaaaaaaaa"); // PUSH14
    let test_bytecode = format!("{test_bytecode}6Eaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"); // PUSH15
    let test_bytecode = format!("{test_bytecode}6Faaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"); // PUSH16
    let test_bytecode = format!("{test_bytecode}70aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"); // PUSH17
    let test_bytecode = format!("{test_bytecode}71aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"); // PUSH18
    let test_bytecode = format!("{test_bytecode}72aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"); // PUSH19
    let test_bytecode = format!("{test_bytecode}73aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"); // PUSH20
    let test_bytecode = format!("{test_bytecode}74aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"); // PUSH21
    let test_bytecode = format!("{test_bytecode}75aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"); // PUSH22
    let test_bytecode = format!("{test_bytecode}76aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"); // PUSH23
    let test_bytecode =
        format!("{test_bytecode}77aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"); // PUSH24
    let test_bytecode =
        format!("{test_bytecode}78aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"); // PUSH25
    let test_bytecode =
        format!("{test_bytecode}79aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"); // PUSH26
    let test_bytecode =
        format!("{test_bytecode}7Aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"); // PUSH27
    let test_bytecode =
        format!("{test_bytecode}7Baaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"); // PUSH28
    let test_bytecode =
        format!("{test_bytecode}7Caaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"); // PUSH29
    let test_bytecode =
        format!("{test_bytecode}7Daaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"); // PUSH30
    let test_bytecode =
        format!("{test_bytecode}7Eaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"); // PUSH31
    let test_bytecode = format!(
        "{test_bytecode}7Faaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    ); // PUSH32
    let test_bytecode = format!("{test_bytecode}80"); // DUP1
    let test_bytecode = format!("{test_bytecode}81"); // DUP2
    let test_bytecode = format!("{test_bytecode}82"); // DUP3
    let test_bytecode = format!("{test_bytecode}83"); // DUP4
    let test_bytecode = format!("{test_bytecode}84"); // DUP5
    let test_bytecode = format!("{test_bytecode}85"); // DUP6
    let test_bytecode = format!("{test_bytecode}86"); // DUP7
    let test_bytecode = format!("{test_bytecode}87"); // DUP8
    let test_bytecode = format!("{test_bytecode}88"); // DUP9
    let test_bytecode = format!("{test_bytecode}89"); // DUP10
    let test_bytecode = format!("{test_bytecode}8A"); // DUP11
    let test_bytecode = format!("{test_bytecode}8B"); // DUP12
    let test_bytecode = format!("{test_bytecode}8C"); // DUP13
    let test_bytecode = format!("{test_bytecode}8D"); // DUP14
    let test_bytecode = format!("{test_bytecode}8E"); // DUP15
    let test_bytecode = format!("{test_bytecode}8F"); // DUP16
    let test_bytecode = format!("{test_bytecode}90"); // SWAP1
    let test_bytecode = format!("{test_bytecode}91"); // SWAP2
    let test_bytecode = format!("{test_bytecode}92"); // SWAP3
    let test_bytecode = format!("{test_bytecode}93"); // SWAP4
    let test_bytecode = format!("{test_bytecode}94"); // SWAP5
    let test_bytecode = format!("{test_bytecode}95"); // SWAP6
    let test_bytecode = format!("{test_bytecode}96"); // SWAP7
    let test_bytecode = format!("{test_bytecode}97"); // SWAP8
    let test_bytecode = format!("{test_bytecode}98"); // SWAP9
    let test_bytecode = format!("{test_bytecode}99"); // SWAP10
    let test_bytecode = format!("{test_bytecode}9A"); // SWAP11
    let test_bytecode = format!("{test_bytecode}9B"); // SWAP12
    let test_bytecode = format!("{test_bytecode}9C"); // SWAP13
    let test_bytecode = format!("{test_bytecode}9D"); // SWAP14
    let test_bytecode = format!("{test_bytecode}9E"); // SWAP15
    let test_bytecode = format!("{test_bytecode}9F"); // SWAP16
    let test_bytecode = format!("{test_bytecode}A0"); // LOG0
    let test_bytecode = format!("{test_bytecode}A1"); // LOG1
    let test_bytecode = format!("{test_bytecode}A2"); // LOG2
    let test_bytecode = format!("{test_bytecode}A3"); // LOG3
    let test_bytecode = format!("{test_bytecode}A4"); // LOG4
    let test_bytecode = format!("{test_bytecode}F0"); // CREATE
    let test_bytecode = format!("{test_bytecode}F1"); // CALL
    let test_bytecode = format!("{test_bytecode}F2"); // CALLCODE
    let test_bytecode = format!("{test_bytecode}F3"); // RETURN
    let test_bytecode = format!("{test_bytecode}F4"); // DELEGATECALL
    let test_bytecode = format!("{test_bytecode}F5"); // CREATE2
    let test_bytecode = format!("{test_bytecode}FA"); // STATICCALL
    let test_bytecode = format!("{test_bytecode}FD"); // REVERT
    let test_bytecode = format!("{test_bytecode}FE"); // INVALID
    let test_bytecode = format!("{test_bytecode}FF"); // SELFDESTRUCT

    test_bytecode
}

mod tests {
    use super::{filter_bytecode_bytes, get_test_bytecode};
    use reth_primitives::{hex::FromHex, Bytes};

    #[test]
    fn test_filter_bytecode_bytes() {
        let test_bytecode_bytes = Bytes::from_hex(get_test_bytecode()).unwrap();
        // assuming push data inside test bytes is "0xaa"
        let manually_filtered_bytes =
            Bytes::from_iter(test_bytecode_bytes.iter().filter(|op| **op != 0xaa));
        assert_eq!(manually_filtered_bytes, filter_bytecode_bytes(&test_bytecode_bytes))
    }
}
