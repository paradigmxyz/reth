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

#[derive(Clone)]
struct AppState {
    info: VizInfo,
    owner_map: Arc<RwLock<Vec<u8>>>,
    subscribers: Arc<AtomicUsize>,
    event_tx: broadcast::Sender<Vec<u8>>,
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

pub async fn start_viz_server(
    config: VizConfig,
    rx: mpsc::Receiver<Vec<PageEvent>>,
) -> std::io::Result<()> {
    let (event_tx, _) = broadcast::channel::<Vec<u8>>(256);

    let owner_map = Arc::new(RwLock::new(config.owner_map));
    let subscribers = Arc::new(AtomicUsize::new(0));

    let state = AppState {
        info: VizInfo {
            page_count: config.page_count,
            page_size: config.page_size,
            dbi_names: config.dbi_names,
        },
        owner_map: owner_map.clone(),
        subscribers: subscribers.clone(),
        event_tx: event_tx.clone(),
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

    let app = Router::new()
        .route("/", get(index_handler))
        .route("/ws", get(ws_handler))
        .route("/api/info", get(info_handler))
        .route("/api/owner_map", get(owner_map_handler))
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
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }

    state.subscribers.fetch_sub(1, Ordering::Relaxed);
}
