use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PageOp {
    Read = 1,
    Write = 2,
    Free = 3,
    BlockStart = 4,
    BlockEnd = 5,
}

impl PageOp {
    fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(Self::Read),
            2 => Some(Self::Write),
            3 => Some(Self::Free),
            4 => Some(Self::BlockStart),
            5 => Some(Self::BlockEnd),
            _ => None,
        }
    }

    pub fn is_block_marker(self) -> bool {
        matches!(self, Self::BlockStart | Self::BlockEnd)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PageEvent {
    pub pgno: u32,
    pub dbi: u32,
    pub op: PageOp,
}

impl PageEvent {
    fn decode(raw: u64) -> Option<Self> {
        let op_byte = (raw >> 56) as u8;
        let op = PageOp::from_u8(op_byte)?;
        let dbi = ((raw >> 32) & 0x00FF_FFFF) as u32;
        let pgno = raw as u32;
        Some(Self { pgno, dbi, op })
    }
}

unsafe extern "C" {
    fn mdbx_pageviz_create() -> *mut std::ffi::c_void;
    fn mdbx_pageviz_destroy(state: *mut std::ffi::c_void);
    fn mdbx_pageviz_enable(state: *mut std::ffi::c_void);
    fn mdbx_pageviz_disable(state: *mut std::ffi::c_void);
    fn mdbx_pageviz_drain(
        state: *mut std::ffi::c_void,
        ring_idx: u32,
        out_buf: *mut u64,
        max_count: u32,
    ) -> u32;
    fn mdbx_pageviz_ring_count(state: *mut std::ffi::c_void) -> u32;
    fn mdbx_pageviz_dropped(state: *mut std::ffi::c_void, ring_idx: u32) -> u64;
}

#[derive(Debug)]
pub struct PageVizStats {
    pub ring_count: u32,
    pub total_dropped: u64,
}

#[derive(Debug, Clone, Copy)]
struct StatePtr(*mut std::ffi::c_void);

// SAFETY: The C state is thread-safe (uses atomics internally).
unsafe impl Send for StatePtr {}
unsafe impl Sync for StatePtr {}

#[derive(Debug)]
pub struct PageVizDrainer {
    state: StatePtr,
    running: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

const DRAIN_BUF_LEN: usize = 4096;
const OVERLOAD_THRESHOLD: usize = 200_000;
const READ_SAMPLE_RATE: usize = 8;

impl PageVizDrainer {
    pub fn new() -> Self {
        let state = StatePtr(unsafe { mdbx_pageviz_create() });
        Self { state, running: Arc::new(AtomicBool::new(false)), handle: None }
    }

    pub fn enable(&self) {
        unsafe { mdbx_pageviz_enable(self.state.0) }
    }

    pub fn disable(&self) {
        unsafe { mdbx_pageviz_disable(self.state.0) }
    }

    pub fn start(&mut self, tick_hz: u32) -> mpsc::Receiver<Vec<PageEvent>> {
        let (tx, rx) = mpsc::channel();
        let running = self.running.clone();
        running.store(true, Ordering::SeqCst);

        let state_addr = self.state.0 as usize;
        let tick_interval = Duration::from_micros(1_000_000 / u64::from(tick_hz));

        let handle = std::thread::Builder::new()
            .name("mdbx-pageviz".into())
            .spawn(move || {
                let state = state_addr as *mut std::ffi::c_void;
                let mut buf = [0u64; DRAIN_BUF_LEN];
                let mut coalesced: HashMap<u32, PageEvent> = HashMap::new();
                let mut markers: Vec<PageEvent> = Vec::new();
                let mut read_counter: usize = 0;

                while running.load(Ordering::Relaxed) {
                    let tick_start = Instant::now();

                    let ring_count = unsafe { mdbx_pageviz_ring_count(state) };

                    for ring_idx in 0..ring_count {
                        let dropped = unsafe { mdbx_pageviz_dropped(state, ring_idx) };
                        if dropped > 0 {
                            tracing::warn!(
                                target: "libmdbx::pageviz",
                                ring_idx,
                                dropped,
                                "pageviz ring dropped events"
                            );
                        }

                        loop {
                            let count = unsafe {
                                mdbx_pageviz_drain(
                                    state,
                                    ring_idx,
                                    buf.as_mut_ptr(),
                                    DRAIN_BUF_LEN as u32,
                                )
                            };
                            if count == 0 {
                                break;
                            }
                            for &raw in &buf[..count as usize] {
                                if let Some(evt) = PageEvent::decode(raw) {
                                    if evt.op.is_block_marker() {
                                        markers.push(evt);
                                    } else {
                                        coalesced.insert(evt.pgno, evt);
                                    }
                                }
                            }
                        }
                    }

                    if !coalesced.is_empty() || !markers.is_empty() {
                        let mut events: Vec<PageEvent> = if coalesced.len() > OVERLOAD_THRESHOLD {
                            coalesced
                                .drain()
                                .filter(|(_, evt)| {
                                    if evt.op == PageOp::Read {
                                        read_counter += 1;
                                        read_counter % READ_SAMPLE_RATE == 0
                                    } else {
                                        true
                                    }
                                })
                                .map(|(_, evt)| evt)
                                .collect()
                        } else {
                            coalesced.drain().map(|(_, evt)| evt).collect()
                        };

                        events.extend(markers.drain(..));

                        if tx.send(events).is_err() {
                            break;
                        }
                    }

                    let elapsed = tick_start.elapsed();
                    if let Some(remaining) = tick_interval.checked_sub(elapsed) {
                        std::thread::sleep(remaining);
                    }
                }
            })
            .expect("failed to spawn mdbx-pageviz thread");

        self.handle = Some(handle);
        rx
    }

    pub fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }

    pub fn stats(&self) -> PageVizStats {
        let ring_count = unsafe { mdbx_pageviz_ring_count(self.state.0) };
        let mut total_dropped = 0u64;
        for ring_idx in 0..ring_count {
            total_dropped += unsafe { mdbx_pageviz_dropped(self.state.0, ring_idx) };
        }
        PageVizStats { ring_count, total_dropped }
    }
}

impl Drop for PageVizDrainer {
    fn drop(&mut self) {
        self.stop();
        unsafe {
            mdbx_pageviz_disable(self.state.0);
            mdbx_pageviz_destroy(self.state.0);
        }
    }
}
