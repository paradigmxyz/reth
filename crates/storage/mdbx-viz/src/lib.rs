use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    mpsc, Arc, RwLock,
};

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    http::{header, StatusCode},
    response::{Html, IntoResponse},
    routing::get,
    Router,
};
use reth_libmdbx::pageviz::{PageEvent, PageOp};
use serde::Serialize;
use tokio::sync::broadcast;

pub mod walker;

#[derive(Clone, Default, Serialize)]
struct ResidencyStats {
    total_pages: u64,
    cached_pages: u64,
    pct: f64,
    per_table: Vec<TableResidency>,
}

#[derive(Clone, Default, Serialize)]
struct TableResidency {
    dbi: usize,
    name: String,
    total: u64,
    cached: u64,
    pct: f64,
}

#[derive(Clone)]
struct AppState {
    info: VizInfo,
    owner_map: Arc<RwLock<Vec<u8>>>,
    tree_info: Arc<Vec<walker::TreeInfo>>,
    subscribers: Arc<AtomicUsize>,
    event_tx: broadcast::Sender<Vec<u8>>,
    residency: Arc<RwLock<Vec<u8>>>,
    residency_stats: Arc<RwLock<ResidencyStats>>,
    residency_tx: broadcast::Sender<Vec<u8>>,
    residency_subscribers: Arc<AtomicUsize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct VizInfo {
    pub page_count: u64,
    pub page_size: u32,
    pub dbi_names: Vec<String>,
}

pub struct VizConfig {
    pub port: u16,
    pub page_count: u64,
    pub page_size: u32,
    pub dbi_names: Vec<String>,
    pub owner_map: Vec<u8>,
    pub tree_info: Vec<walker::TreeInfo>,
    pub db_path: PathBuf,
}

const WIRE_EVENT_SIZE: usize = 8;

fn encode_events(events: &[PageEvent]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(events.len() * WIRE_EVENT_SIZE);
    for ev in events {
        buf.extend_from_slice(&ev.pgno.to_le_bytes());
        buf.extend_from_slice(&(ev.dbi as u16).to_le_bytes());
        buf.push(ev.op as u8);
        buf.push(0);
    }
    buf
}

struct ReadOnlyMapping {
    base: *mut libc::c_void,
    len: usize,
}

impl ReadOnlyMapping {
    fn new(path: &std::path::Path) -> Option<Self> {
        use std::os::unix::io::AsRawFd;
        let file = std::fs::File::open(path).ok()?;
        let len = file.metadata().ok()?.len() as usize;
        if len == 0 {
            return None;
        }
        let base = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        if base == libc::MAP_FAILED {
            return None;
        }
        Some(Self { base, len })
    }

    fn remap(&mut self, path: &std::path::Path) -> bool {
        use std::os::unix::io::AsRawFd;
        let file = match std::fs::File::open(path) {
            Ok(f) => f,
            Err(_) => return false,
        };
        let new_len = match file.metadata() {
            Ok(m) => m.len() as usize,
            Err(_) => return false,
        };
        if new_len == 0 {
            return false;
        }
        unsafe {
            libc::munmap(self.base, self.len);
        }
        let base = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                new_len,
                libc::PROT_READ,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        if base == libc::MAP_FAILED {
            return false;
        }
        self.base = base;
        self.len = new_len;
        true
    }
}

impl Drop for ReadOnlyMapping {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(self.base, self.len);
        }
    }
}

unsafe impl Send for ReadOnlyMapping {}

pub async fn start_viz_server(
    config: VizConfig,
    rx: mpsc::Receiver<Vec<PageEvent>>,
) -> std::io::Result<()> {
    let (event_tx, _) = broadcast::channel::<Vec<u8>>(256);
    let (residency_tx, _) = broadcast::channel::<Vec<u8>>(64);

    let owner_map = Arc::new(RwLock::new(config.owner_map));
    let subscribers = Arc::new(AtomicUsize::new(0));

    let state = AppState {
        info: VizInfo {
            page_count: config.page_count,
            page_size: config.page_size,
            dbi_names: config.dbi_names,
        },
        owner_map: owner_map.clone(),
        tree_info: Arc::new(config.tree_info),
        subscribers: subscribers.clone(),
        event_tx: event_tx.clone(),
        residency: Arc::new(RwLock::new(Vec::new())),
        residency_stats: Arc::new(RwLock::new(ResidencyStats::default())),
        residency_tx: residency_tx.clone(),
        residency_subscribers: Arc::new(AtomicUsize::new(0)),
    };

    let bridge_tx = event_tx.clone();
    std::thread::Builder::new()
        .name("viz-bridge".into())
        .spawn(move || {
            while let Ok(events) = rx.recv() {
                if events.is_empty() {
                    continue;
                }

                {
                    let mut map = owner_map.write().unwrap();
                    for ev in &events {
                        let pg = ev.pgno as usize;
                        if ev.op == PageOp::Write && ev.dbi > 0 && ev.dbi < 0xFE && pg < map.len()
                        {
                            map[pg] = ev.dbi as u8;
                        }
                    }
                }

                if subscribers.load(Ordering::Relaxed) > 0 {
                    let encoded = encode_events(&events);
                    let _ = bridge_tx.send(encoded);
                }
            }
        })
        .expect("failed to spawn viz-bridge thread");

    let res_map = state.residency.clone();
    let res_stats = state.residency_stats.clone();
    let res_tx = state.residency_tx.clone();
    let res_subs = state.residency_subscribers.clone();
    let own_map = state.owner_map.clone();
    let info_clone = state.info.clone();
    let db_path = config.db_path;
    let mdbx_ps = config.page_size;
    let sys_ps = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as u32;

    std::thread::Builder::new()
        .name("residency-poller".into())
        .spawn(move || {
            let mut mapping: Option<ReadOnlyMapping> = None;
            let mut prev: Vec<u8> = Vec::new();

            if db_path.as_os_str().is_empty() {
                tracing::warn!("residency-poller: no db_path configured, residency polling disabled");
                return;
            }

            loop {
                std::thread::sleep(std::time::Duration::from_secs(3));

                if res_subs.load(Ordering::Relaxed) == 0 {
                    continue;
                }

                if mapping.is_none() {
                    match ReadOnlyMapping::new(&db_path) {
                        Some(m) => {
                            tracing::info!(
                                "residency-poller: mapped {} ({} bytes, mdbx_ps={}, sys_ps={})",
                                db_path.display(),
                                m.len,
                                mdbx_ps,
                                sys_ps,
                            );
                            mapping = Some(m);
                        }
                        None => {
                            tracing::warn!(
                                "residency-poller: failed to mmap {}",
                                db_path.display()
                            );
                            continue;
                        }
                    }
                }

                let file_len = match std::fs::metadata(&db_path) {
                    Ok(m) => m.len() as usize,
                    Err(_) => continue,
                };
                let m = mapping.as_mut().unwrap();
                if file_len > m.len {
                    tracing::info!(
                        "residency-poller: file grew {} -> {}, remapping",
                        m.len,
                        file_len,
                    );
                    if !m.remap(&db_path) {
                        tracing::warn!("residency-poller: remap failed");
                        mapping = None;
                        continue;
                    }
                }

                let m = mapping.as_ref().unwrap();
                let base = m.base;
                let len = m.len;

                if mdbx_ps == 0 || sys_ps == 0 || len == 0 {
                    continue;
                }

                let sys_pages = (len + sys_ps as usize - 1) / sys_ps as usize;
                let mdbx_page_count = len / mdbx_ps as usize;

                let mut mincore_vec = vec![0u8; sys_pages];
                let ret = unsafe { libc::mincore(base, len, mincore_vec.as_mut_ptr()) };
                if ret != 0 {
                    continue;
                }

                let mut cur = vec![0u8; mdbx_page_count];
                if mdbx_ps == sys_ps {
                    for i in 0..mdbx_page_count.min(sys_pages) {
                        cur[i] = mincore_vec[i] & 1;
                    }
                } else if mdbx_ps > sys_ps {
                    let ratio = mdbx_ps as usize / sys_ps as usize;
                    for i in 0..mdbx_page_count {
                        let base_idx = i * ratio;
                        let mut all_resident = true;
                        for j in 0..ratio {
                            if base_idx + j >= sys_pages
                                || (mincore_vec[base_idx + j] & 1) == 0
                            {
                                all_resident = false;
                                break;
                            }
                        }
                        cur[i] = if all_resident { 1 } else { 0 };
                    }
                } else {
                    let ratio = sys_ps as usize / mdbx_ps as usize;
                    for i in 0..mdbx_page_count {
                        let sys_idx = i / ratio;
                        cur[i] = if sys_idx < sys_pages {
                            mincore_vec[sys_idx] & 1
                        } else {
                            0
                        };
                    }
                }

                {
                    let mut rm = res_map.write().unwrap();
                    rm.clear();
                    rm.extend_from_slice(&cur);
                }

                {
                    let omap = own_map.read().unwrap();
                    let max_dbi = info_clone.dbi_names.len();
                    let mut total_per = vec![0u64; max_dbi];
                    let mut cached_per = vec![0u64; max_dbi];
                    let mut total_cached = 0u64;
                    let limit = mdbx_page_count.min(omap.len());

                    for i in 0..limit {
                        let dbi = omap[i] as usize;
                        if dbi < max_dbi {
                            total_per[dbi] += 1;
                            if cur[i] == 1 {
                                cached_per[dbi] += 1;
                                total_cached += 1;
                            }
                        }
                    }

                    let mut stats = res_stats.write().unwrap();
                    stats.total_pages = mdbx_page_count as u64;
                    stats.cached_pages = total_cached;
                    stats.pct = if mdbx_page_count > 0 {
                        (total_cached as f64 / mdbx_page_count as f64) * 100.0
                    } else {
                        0.0
                    };

                    stats.per_table = (0..max_dbi)
                        .filter(|&d| total_per[d] > 0)
                        .map(|d| TableResidency {
                            dbi: d,
                            name: info_clone.dbi_names.get(d).cloned().unwrap_or_default(),
                            total: total_per[d],
                            cached: cached_per[d],
                            pct: if total_per[d] > 0 {
                                (cached_per[d] as f64 / total_per[d] as f64) * 100.0
                            } else {
                                0.0
                            },
                        })
                        .collect();
                }

                if prev.len() != cur.len() {
                    let mut buf = Vec::with_capacity(1 + cur.len());
                    buf.push(0u8);
                    buf.extend_from_slice(&cur);
                    let _ = res_tx.send(buf);
                    tracing::info!(
                        "residency-poller: broadcast full snapshot ({} pages)",
                        cur.len(),
                    );
                } else {
                    let mut spans: Vec<u8> = Vec::new();
                    let mut span_count: u32 = 0;
                    spans.extend_from_slice(&[1u8]);
                    spans.extend_from_slice(&[0u8; 4]);

                    let mut i = 0;
                    while i < cur.len() {
                        if cur[i] != prev[i] {
                            let state_val = cur[i];
                            let start = i as u32;
                            let mut end = i + 1;
                            while end < cur.len()
                                && cur[end] != prev[end]
                                && cur[end] == state_val
                            {
                                end += 1;
                            }
                            let run_len = (end - i) as u32;
                            spans.extend_from_slice(&start.to_le_bytes());
                            spans.extend_from_slice(&run_len.to_le_bytes());
                            spans.push(state_val);
                            span_count += 1;
                            i = end;
                        } else {
                            i += 1;
                        }
                    }

                    if span_count > 0 {
                        spans[1..5].copy_from_slice(&span_count.to_le_bytes());
                        let _ = res_tx.send(spans);
                    }
                }

                prev = cur;
            }
        })
        .expect("failed to spawn residency-poller thread");

    let app = Router::new()
        .route("/", get(index_handler))
        .route("/ws", get(ws_handler))
        .route("/api/info", get(info_handler))
        .route("/api/owner_map", get(owner_map_handler))
        .route("/api/tree_info", get(tree_info_handler))
        .route("/api/residency", get(residency_handler))
        .route("/api/residency_stats", get(residency_stats_handler))
        .route("/ws_residency", get(ws_residency_handler))
        .with_state(state);

    let addr = format!("0.0.0.0:{}", config.port);
    tracing::info!("mdbx-viz server listening on {addr}");

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn index_handler() -> Html<&'static str> {
    Html(include_str!("index.html"))
}

async fn info_handler(State(state): State<AppState>) -> impl IntoResponse {
    axum::Json(state.info)
}

async fn owner_map_handler(State(state): State<AppState>) -> impl IntoResponse {
    let map = state.owner_map.read().unwrap();
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/octet-stream")],
        map.clone(),
    )
        .into_response()
}

async fn tree_info_handler(State(state): State<AppState>) -> impl IntoResponse {
    axum::Json(state.tree_info.as_ref().clone())
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws(socket, state))
}

async fn handle_ws(mut socket: WebSocket, state: AppState) {
    state.subscribers.fetch_add(1, Ordering::Relaxed);
    let mut rx = state.event_tx.subscribe();

    loop {
        match rx.recv().await {
            Ok(data) => {
                if socket.send(Message::Binary(data.into())).await.is_err() {
                    break;
                }
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!("ws client lagged by {n} messages");
                let msg = format!("{{\"lagged\":{n}}}");
                if socket.send(Message::Text(msg)).await.is_err() {
                    break;
                }
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }

    state.subscribers.fetch_sub(1, Ordering::Relaxed);
}

async fn residency_handler(State(state): State<AppState>) -> impl IntoResponse {
    let map = state.residency.read().unwrap();
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/octet-stream")],
        map.clone(),
    )
        .into_response()
}

async fn residency_stats_handler(State(state): State<AppState>) -> impl IntoResponse {
    let stats = state.residency_stats.read().unwrap();
    axum::Json(stats.clone())
}

async fn ws_residency_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws_residency(socket, state))
}

async fn handle_ws_residency(mut socket: WebSocket, state: AppState) {
    state.residency_subscribers.fetch_add(1, Ordering::Relaxed);

    {
        let msg = {
            let map = state.residency.read().unwrap();
            if !map.is_empty() {
                let mut buf = Vec::with_capacity(1 + map.len());
                buf.push(0u8);
                buf.extend_from_slice(&map);
                Some(buf)
            } else {
                None
            }
        };
        if let Some(msg) = msg {
            let _ = socket.send(Message::Binary(msg.into())).await;
        }
    }

    let mut rx = state.residency_tx.subscribe();
    loop {
        match rx.recv().await {
            Ok(data) => {
                if socket.send(Message::Binary(data.into())).await.is_err() {
                    break;
                }
            }
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }

    state.residency_subscribers.fetch_sub(1, Ordering::Relaxed);
}
