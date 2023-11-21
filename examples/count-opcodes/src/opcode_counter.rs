use comfy_table::{Cell, Row, Table};
use reth_primitives::Bytes;
use reth_revm::interpreter::OpCode;
use std::collections::HashMap;

macro_rules! opcodes_tuples_vec {
    ($map:expr) => {{
        let mut vec = $map
            .iter()
            .map(|(k, v)| {
                let mut string = String::new();
                (0..k.len())
                    .for_each(|i| string.push_str(&format!("{} ", &opcode_or_invalid(k[i]))));
                (string, *v)
            })
            .collect::<Vec<_>>();
        vec.sort_unstable_by_key(|(_str, occurrecies)| *occurrecies);
        vec
    }};
}

pub(crate) struct OpCodeCounter {
    pub opcodes: HashMap<[u8; 1], usize>,
    pub couples_opcodes: HashMap<[u8; 2], usize>,
    pub triplets_opcodes: HashMap<[u8; 3], usize>,
    pub quadruplets_opcodes: HashMap<[u8; 4], usize>,
}

impl OpCodeCounter {
    pub(crate) fn new() -> Self {
        Self {
            opcodes: HashMap::new(),
            couples_opcodes: HashMap::new(),
            triplets_opcodes: HashMap::new(),
            quadruplets_opcodes: HashMap::new(),
        }
    }

    pub(crate) fn count_sequences(&mut self, bytes: &Bytes) {
        for (i, opcode) in bytes.iter().enumerate() {
            *self.opcodes.entry([*opcode]).or_default() += 1;
            if let Some(&[a, b]) = bytes.get(i..=i + 1) {
                *self.couples_opcodes.entry([a, b]).or_default() += 1;
            }
            if let Some(&[a, b, c]) = bytes.get(i..=i + 2) {
                *self.triplets_opcodes.entry([a, b, c]).or_default() += 1;
            }
            if let Some(&[a, b, c, d]) = bytes.get(i..=i + 3) {
                *self.quadruplets_opcodes.entry([a, b, c, d]).or_default() += 1;
            }
        }
    }

    pub(crate) fn print_counts(&self, take: usize) {
        let opcodes_vec = opcodes_tuples_vec!(&self.opcodes);
        let couples_vec = opcodes_tuples_vec!(&self.couples_opcodes);
        let triplets_vec = opcodes_tuples_vec!(&self.triplets_opcodes);
        let quadruplets_vec = opcodes_tuples_vec!(&self.quadruplets_opcodes);

        print_opcode_table("Opcodes", &opcodes_vec[..take]);
        print_opcode_table("Opcodes couples", &couples_vec[..take]);
        print_opcode_table("Opcodes triplets", &triplets_vec[..take]);
        print_opcode_table("Opcodes quadruplets", &quadruplets_vec[..take]);
    }
}

/// Utility function for creating an opcode. When it finds an unknown opcode it creates the
/// `INVALID` opcode.
fn opcode_or_invalid(opcode: u8) -> OpCode {
    if let Some(op) = OpCode::new(opcode) {
        op
    } else {
        OpCode::new(0xFE).unwrap() // INVALID
    }
}

/// Generic function to print opcode sequences.
fn print_opcode_table<T: ToString>(header: &str, sequences: &[(T, usize)]) {
    let mut table = Table::new();
    table.load_preset(comfy_table::presets::ASCII_MARKDOWN);
    table.set_header([header, "Occurrences"]);

    sequences.iter().for_each(|(seq, count)| {
        let mut row = Row::new();
        row.add_cell(Cell::new(seq.to_string()));
        row.add_cell(Cell::new(count));
        table.add_row(row);
    });

    println!("\n{}", table);
}
