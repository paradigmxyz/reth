//! Runs the production sidecar writer, decoder, and recovery against a volatile/durable file.
//! Header publication is modeled as an atomic committed-length update; this does not simulate
//! `NippyJar` configuration writes, MDBX, or `RocksDB`. The sidecar has no record checksums, so
//! recovery can reject missing committed records but cannot detect arbitrary corruption within a
//! full record.

use super::*;
use commonware_runtime::{deterministic, Clock, Runner, Spawner, Supervisor};
use std::{
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

#[derive(Clone, Debug, Default)]
struct Storage(Arc<Mutex<State>>);

impl Storage {
    fn open(&self) -> MemoryFile {
        MemoryFile { storage: self.clone(), generation: self.0.lock().unwrap().generation }
    }

    fn arm(&self, fault: Option<Fault>) {
        self.0.lock().unwrap().fault = fault;
    }

    fn publish(&self, len: u64) {
        let mut state = self.0.lock().unwrap();
        state.committed_len = len;
        state.trace.push(Event::Publish(len));
    }

    fn committed_len(&self) -> u64 {
        self.0.lock().unwrap().committed_len
    }

    /// Unsynced appends can leave a prefix on disk. Unsynced truncation can survive or be lost.
    /// Synced bytes are preserved, and previous handles cannot mutate the recovered generation.
    fn crash(&self, retention: Retention) {
        let mut state = self.0.lock().unwrap();
        let visible = state.visible.clone();
        match retention {
            Retention::None => {}
            Retention::All => state.durable = visible,
            Retention::Prefix(bytes) => {
                let start = state.durable.len();
                if visible.len() > start {
                    let end = (start + bytes).min(visible.len());
                    state.durable.extend_from_slice(&visible[start..end]);
                }
            }
        }
        state.visible = state.durable.clone();
        state.generation += 1;
        state.fault = None;
        let image = state.durable.clone();
        state.trace.push(Event::Crash(image));
    }

    fn corrupt_length(&self, len: usize) {
        let mut state = self.0.lock().unwrap();
        state.durable.truncate(len);
        state.visible = state.durable.clone();
        state.trace.push(Event::CorruptLength(len));
    }

    fn assert_contents(&self, expected: &[ChangesetOffset]) {
        let file = self.open();
        assert_eq!(file.byte_len().unwrap(), (expected.len() * RECORD_SIZE) as u64);
        let actual = (0..expected.len())
            .map(|index| read_offset(&file, index as u64).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(actual, expected);
    }

    fn trace(&self) -> Vec<Event> {
        self.0.lock().unwrap().trace.clone()
    }
}

#[derive(Debug, Default)]
struct State {
    visible: Vec<u8>,
    durable: Vec<u8>,
    committed_len: u64,
    generation: u64,
    fault: Option<Fault>,
    trace: Vec<Event>,
}

#[derive(Debug)]
struct MemoryFile {
    storage: Storage,
    generation: u64,
}

impl MemoryFile {
    fn state(&self) -> io::Result<MutexGuard<'_, State>> {
        let state = self.storage.0.lock().unwrap();
        if state.generation != self.generation {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "handle belongs to crashed run"));
        }
        Ok(state)
    }
}

impl SidecarFile for MemoryFile {
    fn byte_len(&self) -> io::Result<u64> {
        Ok(self.state()?.visible.len() as u64)
    }

    fn read_exact_at(&self, bytes: &mut [u8], offset: u64) -> io::Result<()> {
        let state = self.state()?;
        let start = offset as usize;
        let content = state
            .visible
            .get(start..start + bytes.len())
            .ok_or_else(|| io::Error::from(io::ErrorKind::UnexpectedEof))?;
        bytes.copy_from_slice(content);
        Ok(())
    }

    fn append_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        let mut state = self.state()?;
        if let Some(Fault::WritePrefix(prefix)) = state.fault {
            state.fault = None;
            let prefix = prefix.min(bytes.len());
            state.visible.extend_from_slice(&bytes[..prefix]);
            state.trace.push(Event::PartialWrite(prefix));
            return Err(injected());
        }
        state.visible.extend_from_slice(bytes);
        state.trace.push(Event::Write(bytes.to_vec()));
        Ok(())
    }

    fn resize(&self, len: u64) -> io::Result<()> {
        let mut state = self.state()?;
        let fault = match state.fault {
            Some(fault @ (Fault::ResizeBefore | Fault::ResizeAfter)) => {
                state.fault = None;
                Some(fault)
            }
            _ => None,
        };
        state.trace.push(Event::Resize(len, fault));
        if fault == Some(Fault::ResizeBefore) {
            return Err(injected());
        }
        state.visible.resize(len as usize, 0);
        if fault == Some(Fault::ResizeAfter) {
            return Err(injected());
        }
        Ok(())
    }

    fn sync(&self) -> io::Result<()> {
        let mut state = self.state()?;
        let fault = match state.fault {
            Some(fault @ (Fault::SyncBefore | Fault::SyncAfter)) => {
                state.fault = None;
                Some(fault)
            }
            _ => None,
        };
        state.trace.push(Event::Sync(fault));
        if fault == Some(Fault::SyncBefore) {
            return Err(injected());
        }
        state.durable = state.visible.clone();
        if fault == Some(Fault::SyncAfter) {
            return Err(injected());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fault {
    WritePrefix(usize),
    SyncBefore,
    SyncAfter,
    ResizeBefore,
    ResizeAfter,
}

#[derive(Clone, Copy, Debug)]
enum Retention {
    None,
    Prefix(usize),
    All,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Event {
    Write(Vec<u8>),
    PartialWrite(usize),
    Resize(u64, Option<Fault>),
    Sync(Option<Fault>),
    Publish(u64),
    Crash(Vec<u8>),
    CorruptLength(usize),
}

fn injected() -> io::Error {
    io::Error::other("injected sidecar I/O failure")
}

fn records() -> Vec<ChangesetOffset> {
    [(0, 3), (3, 1), (4, 9), (13, 2), (15, 7)]
        .into_iter()
        .map(|(offset, count)| ChangesetOffset::new(offset, count))
        .collect()
}

fn recover(storage: &Storage) -> io::Result<SidecarWriter<MemoryFile>> {
    SidecarWriter::recover(storage.open(), Path::new("simulated.csoff"), storage.committed_len())
}

fn initial_storage(count: usize) -> Storage {
    let storage = Storage::default();
    let mut writer = recover(&storage).unwrap();
    for record in records().iter().take(count) {
        writer.append(record).unwrap();
    }
    writer.sync().unwrap();
    storage.publish(count as u64);
    storage
}

fn runner(seed: u64) -> deterministic::Runner {
    deterministic::Runner::new(
        deterministic::Config::default().with_seed(seed).with_timeout(Some(Duration::from_secs(5))),
    )
}

fn simulate_append(seed: u64, delay: u64, fault: Option<Fault>) -> (String, Vec<Event>) {
    runner(seed).start(|context| async move {
        let storage = initial_storage(2);
        storage.arm(fault);
        let writer_storage = storage.clone();
        let writer = context.child("sidecar_writer").spawn(move |context| async move {
            let mut writer = recover(&writer_storage)?;
            for record in records().iter().skip(2).take(2) {
                let previous_len = writer.records_written;
                if let Err(error) = writer.append(record) {
                    assert_eq!(writer.records_written, previous_len);
                    return Err(error);
                }
                context.sleep(Duration::from_millis(10)).await;
            }
            writer.sync()?;
            context.sleep(Duration::from_millis(10)).await;
            writer_storage.publish(writer.records_written);
            Ok::<_, io::Error>(())
        });

        context.sleep(Duration::from_millis(delay)).await;
        writer.abort();
        // A crash stops the old actor before restarting storage, including after an I/O error.
        let _ = writer.await;
        let retention = match seed % 3 {
            0 => Retention::None,
            1 => Retention::Prefix(1 + seed as usize % (2 * RECORD_SIZE)),
            _ => Retention::All,
        };
        storage.crash(retention);

        let committed = storage.committed_len() as usize;
        assert!(committed == 2 || committed == 4);
        if delay == 100 && fault.is_none() {
            assert_eq!(committed, 4, "a completed writer must publish its synced records");
        }
        let mut recovered = recover(&storage).unwrap();
        assert_eq!(recovered.records_written, committed as u64);
        storage.assert_contents(&records()[..committed]);

        // A repaired sidecar remains appendable, and another crash cannot resurrect its suffix.
        let sentinel = ChangesetOffset::new(1000, seed % 1000 + 1);
        recovered.append(&sentinel).unwrap();
        recovered.sync().unwrap();
        storage.publish(recovered.records_written);
        drop(recovered);
        storage.crash(Retention::None);
        let _recovered = recover(&storage).unwrap();
        let mut expected = records()[..committed].to_vec();
        expected.push(sentinel);
        storage.assert_contents(&expected);
        (context.auditor().state(), storage.trace())
    })
}

fn simulate_recovery(seed: u64) -> (String, Vec<Event>) {
    runner(seed).start(|context| async move {
        let storage = initial_storage(2);
        let mut writer = recover(&storage).unwrap();
        writer.append(&records()[2]).unwrap();
        writer.append(&records()[3]).unwrap();
        storage.arm(Some(Fault::WritePrefix(1 + seed as usize % (RECORD_SIZE - 1))));
        assert!(writer.append(&records()[4]).is_err());
        assert_eq!(writer.records_written, 4);
        drop(writer);
        storage.crash(Retention::All);

        // Crash repeatedly while repairing a partial record. The final attempt must also remove
        // complete but unpublished records. A failed sync may already have made its resize durable.
        for fault in [Fault::ResizeBefore, Fault::ResizeAfter, Fault::SyncBefore, Fault::SyncAfter]
        {
            storage.arm(Some(fault));
            assert!(recover(&storage).is_err(), "fault {fault:?} must interrupt recovery");
            storage.crash(Retention::None);
        }
        let _writer = recover(&storage).unwrap();
        storage.assert_contents(&records()[..2]);
        storage.crash(Retention::None);
        // A successful repair must survive without requiring another healing pass.
        storage.assert_contents(&records()[..2]);
        let _writer = recover(&storage).unwrap();
        storage.assert_contents(&records()[..2]);

        // Structural corruption below the published header must fail instead of silently
        // accepting a shorter committed prefix. Full-record contents have no checksum here.
        storage.corrupt_length(RECORD_SIZE + seed as usize % RECORD_SIZE);
        let error = recover(&storage).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        (context.auditor().state(), storage.trace())
    })
}

fn simulate_truncate(seed: u64, fault: Option<Fault>) -> (String, Vec<Event>) {
    runner(seed).start(|context| async move {
        let storage = initial_storage(4);
        let mut writer = recover(&storage).unwrap();
        // As in unwind, the authoritative header is lowered before destructive sidecar work.
        storage.publish(2);
        storage.arm(fault);
        let result = writer.truncate(2);
        assert_eq!(writer.records_written, if result.is_ok() { 2 } else { 4 });
        drop(writer);
        storage.crash(if seed.is_multiple_of(2) { Retention::None } else { Retention::All });
        if result.is_ok() {
            storage.assert_contents(&records()[..2]);
        }
        let _writer = recover(&storage).unwrap();
        storage.assert_contents(&records()[..2]);
        storage.crash(Retention::None);
        storage.assert_contents(&records()[..2]);
        let _writer = recover(&storage).unwrap();
        storage.assert_contents(&records()[..2]);
        (context.auditor().state(), storage.trace())
    })
}

#[test]
fn deterministic_sidecar_fault_recovery() {
    let seeds: Vec<u64> = match std::env::var("RETH_DST_SEED") {
        Ok(seed) => vec![seed.parse().expect("RETH_DST_SEED must be a u64")],
        Err(std::env::VarError::NotPresent) => (0..16).collect(),
        Err(error) => panic!("invalid RETH_DST_SEED: {error}"),
    };
    for seed in seeds {
        eprintln!("sidecar fault recovery seed={seed}");
        for fault in [
            None,
            Some(Fault::WritePrefix(seed as usize % RECORD_SIZE)),
            Some(Fault::SyncBefore),
            Some(Fault::SyncAfter),
        ] {
            for delay in [0, 10, 20, 30, 100] {
                assert_eq!(
                    simulate_append(seed, delay, fault),
                    simulate_append(seed, delay, fault),
                    "append replay: seed={seed}, delay={delay}, fault={fault:?}"
                );
            }
        }
        assert_eq!(simulate_recovery(seed), simulate_recovery(seed), "recovery replay: {seed}");
        for fault in [
            None,
            Some(Fault::ResizeBefore),
            Some(Fault::ResizeAfter),
            Some(Fault::SyncBefore),
            Some(Fault::SyncAfter),
        ] {
            assert_eq!(
                simulate_truncate(seed, fault),
                simulate_truncate(seed, fault),
                "truncate replay: seed={seed}, fault={fault:?}"
            );
        }
    }
}
