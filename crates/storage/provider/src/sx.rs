//! Experimental: bytecode translated to evm2 superinstructions.
//!
//! When a database enables superinstructions, every legacy bytecode it stores carries a record of
//! its translation next to its jump table (see [`reth_primitives_traits::Bytecode`]), so loading a
//! contract also loads its translation. Builds without superinstructions cannot read such bytecode,
//! so enabling is one-way: the setting is kept in the database's metadata from then on.
//!
//! The first start with superinstructions translates the bytecode already in the database. It
//! splits the key space into [`ROUNDS`] rounds that each commit with the position to continue from,
//! so an interrupted start resumes. Reth's database maps its file without read-ahead, so reading a
//! cold value waits on page faults: each round is read by [`READERS`] threads at once, which keeps
//! the drive busy, and is read and translated while the round before it is written.

use crate::{DBProvider, DatabaseProviderFactory};
use alloy_primitives::B256;
use rayon::prelude::*;
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW},
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_primitives_traits::Bytecode;
use reth_storage_errors::provider::{ProviderError, ProviderResult};
use std::{
    sync::OnceLock,
    time::{Duration, Instant},
};

/// Metadata key of the [`SxState`].
pub const SX_KEY: &str = "experimental_sx";

/// Parts of the key space translated and committed one after another.
const ROUNDS: usize = if cfg!(test) { 4 } else { 256 };

/// Threads reading a round at once.
const READERS: usize = 32;

/// Key-space positions are the first two bytes of a hash.
const POSITIONS: usize = 1 << 16;

/// Positions per round.
const WIDTH: usize = POSITIONS / ROUNDS;

const _: () = assert!(WIDTH.is_multiple_of(READERS) || WIDTH < READERS);

/// Returns the smallest hash at position `p`, or `None` past the end.
fn key(p: usize) -> Option<B256> {
    (p < POSITIONS).then(|| {
        let mut key = B256::ZERO;
        key[..2].copy_from_slice(&(p as u16).to_be_bytes());
        key
    })
}

/// Superinstruction state of a database.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SxState {
    /// Whether the database uses superinstructions. Never cleared once set.
    pub enabled: bool,
    /// Whether the bytecode stored before enabling has been translated.
    pub translated: bool,
    /// The bytecode hash translation of existing bytecode continues from.
    pub next: Option<B256>,
}

impl SxState {
    /// Encodes the state: a flags byte (enabled, translated, has next), then the next hash.
    fn encode(&self) -> Vec<u8> {
        let flags =
            self.enabled as u8 | (self.translated as u8) << 1 | (self.next.is_some() as u8) << 2;
        let mut out = vec![flags];
        if let Some(next) = self.next {
            out.extend_from_slice(next.as_slice());
        }
        out
    }

    fn decode(bytes: &[u8]) -> Option<Self> {
        let (&flags, rest) = bytes.split_first()?;
        let next = match (flags & 4 != 0, rest.len()) {
            (true, 32) => Some(B256::from_slice(rest)),
            (false, 0) => None,
            _ => return None,
        };
        Some(Self { enabled: flags & 1 != 0, translated: flags & 2 != 0, next })
    }
}

/// Reads the superinstruction state of a database; a database without one has them off.
pub fn state<P: DBProvider>(provider: &P) -> ProviderResult<SxState> {
    match provider.tx_ref().get::<tables::Metadata>(SX_KEY.to_string())? {
        None => Ok(SxState::default()),
        Some(bytes) => SxState::decode(&bytes).ok_or_else(|| {
            ProviderError::other(std::io::Error::other("unreadable superinstruction state"))
        }),
    }
}

/// Writes the superinstruction state of a database.
pub fn write_state<P: DBProvider<Tx: DbTxMut>>(
    provider: &P,
    state: &SxState,
) -> ProviderResult<()> {
    Ok(provider.tx_ref().put::<tables::Metadata>(SX_KEY.to_string(), state.encode())?)
}

/// Translates a bytecode as the EVM does when it runs it with superinstructions.
pub type Translator = Box<dyn Fn(&evm2::bytecode::Bytecode) + Send + Sync>;

static TRANSLATOR: OnceLock<Translator> = OnceLock::new();

/// Sets the translator bytecode is translated with before it is written. Set once per process,
/// when the node enables superinstructions; later calls are ignored.
pub fn set_translator(translator: Translator) {
    let _ = TRANSLATOR.set(translator);
}

/// Translates `bytecode` if superinstructions are enabled, so it is stored translated.
#[inline]
pub fn translate(bytecode: &Bytecode) {
    if let Some(translator) = TRANSLATOR.get() {
        translator(&bytecode.0);
    }
}

/// Progress of translating the bytecode already in a database.
#[derive(Debug, Clone, Copy, Default)]
pub struct Progress {
    /// Bytecodes read.
    pub codes: u64,
    /// Bytecodes translated and rewritten.
    pub translated: u64,
    /// Bytes of code read.
    pub bytes: u64,
    /// Time spent.
    pub elapsed: Duration,
    /// Thread time spent reading and decoding bytecode, summed over the readers.
    pub read: Duration,
    /// Thread time spent translating, summed over the readers.
    pub translate: Duration,
    /// Time the writer spent waiting for rounds to be read.
    pub wait: Duration,
    /// Time spent writing translated bytecode.
    pub write: Duration,
    /// Time spent committing.
    pub commit: Duration,
}

/// A round read and translated.
struct Round {
    end: usize,
    codes: Vec<(B256, Bytecode)>,
    read: Duration,
    translate: Duration,
}

/// Translates the bytecode already in the database with the translator of [`set_translator`],
/// continuing where an earlier run stopped, and marks the database translated.
///
/// Calls `progress` after every round.
pub fn translate_existing<F>(
    factory: &F,
    mut progress: impl FnMut(&Progress),
) -> ProviderResult<Progress>
where
    F: DatabaseProviderFactory + Sync,
{
    let translator = TRANSLATOR.get().ok_or_else(|| {
        ProviderError::other(std::io::Error::other("superinstruction translator is not set"))
    })?;
    let start = Instant::now();
    let mut done = Progress::default();
    let mut state = state(&factory.database_provider_ro()?)?;
    if state.translated {
        return Ok(done)
    }
    // An earlier run stopped at a round boundary, or, from before rounds, anywhere in one: redo it.
    let first =
        state.next.map_or(0, |next| u16::from_be_bytes([next[0], next[1]]) as usize / WIDTH);
    std::thread::scope(|scope| {
        let (tx, rx) = std::sync::mpsc::sync_channel::<ProviderResult<Round>>(1);
        scope.spawn(move || {
            for round in first..ROUNDS {
                let read = read_round(factory, translator, round);
                let failed = read.is_err();
                if tx.send(read).is_err() || failed {
                    break
                }
            }
        });
        let mut waited = Instant::now();
        for round in rx {
            done.wait += waited.elapsed();
            let round = round?;
            done.read += round.read;
            done.translate += round.translate;

            let t = Instant::now();
            let provider = factory.database_provider_rw()?;
            let mut cursor = provider.tx_ref().cursor_write::<tables::Bytecodes>()?;
            for (hash, code) in &round.codes {
                done.codes += 1;
                done.bytes += code.0.original_byte_slice().len() as u64;
                if code.0.sx_record().is_some() {
                    cursor.upsert(*hash, code)?;
                    done.translated += 1;
                }
            }
            drop(cursor);
            state.next = key(round.end);
            state.translated = state.next.is_none();
            write_state(&provider, &state)?;
            done.write += t.elapsed();
            let t = Instant::now();
            provider.commit()?;
            done.commit += t.elapsed();

            done.elapsed = start.elapsed();
            progress(&done);
            waited = Instant::now();
        }
        Ok::<_, ProviderError>(())
    })?;
    done.elapsed = start.elapsed();
    Ok(done)
}

/// Reads and translates the bytecode of a round, [`READERS`] parts at once.
fn read_round<F>(factory: &F, translator: &Translator, round: usize) -> ProviderResult<Round>
where
    F: DatabaseProviderFactory + Sync,
{
    let readers = READERS.min(WIDTH);
    let parts = (0..readers)
        .into_par_iter()
        .map(|k| {
            let from = round * WIDTH + k * WIDTH / readers;
            read_part(factory, translator, from, round * WIDTH + (k + 1) * WIDTH / readers)
        })
        .collect::<Vec<_>>();
    let mut out = Round {
        end: (round + 1) * WIDTH,
        codes: Vec::new(),
        read: Duration::ZERO,
        translate: Duration::ZERO,
    };
    for part in parts {
        let part = part?;
        out.codes.extend(part.codes);
        out.read += part.read;
        out.translate += part.translate;
    }
    Ok(out)
}

/// Reads and translates the bytecode whose hashes are at positions `from..to`.
fn read_part<F>(
    factory: &F,
    translator: &Translator,
    from: usize,
    to: usize,
) -> ProviderResult<Round>
where
    F: DatabaseProviderFactory,
{
    use core::ops::Bound;
    let provider = factory.database_provider_ro()?;
    let mut cursor = provider.tx_ref().cursor_read::<tables::Bytecodes>()?;
    let lower = Bound::Included(key(from).expect("in range"));
    let upper = key(to).map_or(Bound::Unbounded, Bound::Excluded);
    let mut walker = cursor.walk_range((lower, upper))?;
    let mut out =
        Round { end: to, codes: Vec::new(), read: Duration::ZERO, translate: Duration::ZERO };
    loop {
        let t = Instant::now();
        let Some(entry) = walker.next() else { break };
        let (hash, code) = entry?;
        out.read += t.elapsed();
        let t = Instant::now();
        translator(&code.0);
        // Encode the record here, in parallel, so the writer only copies it.
        let _ = code.0.sx_record();
        out.translate += t.elapsed();
        out.codes.push((hash, code));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::create_test_provider_factory;
    use alloy_primitives::{hex, keccak256, Bytes};

    #[test]
    fn state_round_trips() {
        for state in [
            SxState::default(),
            SxState { enabled: true, translated: false, next: Some(B256::repeat_byte(7)) },
            SxState { enabled: true, translated: true, next: None },
        ] {
            assert_eq!(SxState::decode(&state.encode()), Some(state));
        }
        assert_eq!(SxState::decode(&[4]), None);
    }

    #[test]
    fn translates_existing_bytecode_in_batches() {
        let mut config = evm2::ExecutionConfig::<evm2::BaseEvmTypes>::for_spec_and_version(
            evm2::SpecId::OSAKA,
            evm2::Version::new(evm2::SpecId::OSAKA),
        );
        config.set_superinstructions(true);
        set_translator(Box::new(move |code| config.analyze(code)));

        let factory = create_test_provider_factory();
        // A Solidity prologue with a different selector each, and code nothing fuses in.
        let prologue = hex!("608060405234801561000f575f80fd5b5060043610610029575f3560e01c8063a9059cbb1461002d575b5f80fd5b00");
        let mut codes = Vec::new();
        for i in 0..30u8 {
            let mut code = prologue.to_vec();
            code[35] = i;
            codes.push(Bytes::from(code));
        }
        codes.push(Bytes::from_static(&[0x00]));
        let provider = factory.database_provider_rw().unwrap();
        for code in &codes {
            provider
                .tx_ref()
                .put::<tables::Bytecodes>(keccak256(code), Bytecode::new_raw(code.clone()))
                .unwrap();
        }
        write_state(&provider, &SxState { enabled: true, ..Default::default() }).unwrap();
        provider.commit().unwrap();

        // Resume halfway: only the last two rounds are left.
        let provider = factory.database_provider_rw().unwrap();
        write_state(&provider, &SxState { enabled: true, translated: false, next: key(2 * WIDTH) })
            .unwrap();
        provider.commit().unwrap();
        let mut rounds = 0;
        let done = translate_existing(&factory, |_| rounds += 1).unwrap();
        let upper = codes.iter().filter(|c| keccak256(c)[0] >= 0x80).count() as u64;
        assert_eq!((done.codes, done.translated, rounds), (upper, upper, 2));

        let provider = factory.database_provider_rw().unwrap();
        write_state(&provider, &SxState { enabled: true, ..Default::default() }).unwrap();
        provider.commit().unwrap();
        let mut rounds = 0;
        let done = translate_existing(&factory, |_| rounds += 1).unwrap();
        // Code nothing fuses in is stored translated too, with an empty record.
        assert_eq!((done.codes, done.translated), (31, 31));
        assert_eq!(rounds, 4);
        let provider = factory.database_provider_ro().unwrap();
        assert_eq!(
            state(&provider).unwrap(),
            SxState { enabled: true, translated: true, next: None }
        );
        for code in &codes {
            let stored =
                provider.tx_ref().get::<tables::Bytecodes>(keccak256(code)).unwrap().unwrap();
            assert!(stored.0.sx_record().is_some(), "stored translated");
            assert_eq!(stored.original_bytes(), *code);
        }
        drop(provider);
        // Already translated: nothing to do.
        let mut again = 0;
        assert_eq!(translate_existing(&factory, |_| again += 1).unwrap().codes, 0);
        assert_eq!(again, 0);
    }
}
