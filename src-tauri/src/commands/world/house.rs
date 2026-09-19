//! Open house and visits (plan Phase 9, decision D-13): the host's app is the
//! authority and `omniworld-server` is a relay. Nothing here opens a socket
//! until the user clicks "Open house" or types a code, and closing the house
//! drops the only socket there is.
//!
//! Wire (same document as `omniworld-server/src/main.rs`): one WebSocket at
//! `/v1/room`; JSON text frames for control, binary frames `[opcode][payload]`
//! with `WorldDiff=40`, `WorldInput=41`, `Interest=42`, `Chat=43`. The hello is
//! signed with the ed25519 key of the local profile, so there are no accounts.
//!
//! Host: every blob the route gets is also sent as a `40` frame; a visitor is
//! an entity `1000 + visitor_id` spawned in the host's world, moved by the
//! visitor's `41` frames. Visitor: `40` frames feed the same channel the local
//! world would, so the canvas cannot tell a visit from home.

use std::sync::{Arc, Mutex};

use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tauri::ipc::{Channel, InvokeResponseBody, Response};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

use crate::world_manager::DiffSink;

pub const OP_WORLD_DIFF: u8 = 40;
pub const OP_WORLD_INPUT: u8 = 41;
pub const OP_INTEREST: u8 = 42;
pub const OP_CHAT: u8 = 43;

pub const EVENT_STATE: &str = "house://state";
pub const EVENT_CHAT: &str = "house://chat";
pub const EVENT_CLOSED: &str = "house://closed";

pub const ERR_HOUSE: &str = "ERR_HOUSE";
/// The public room server: `omniworld-server` behind the Caddy that already
/// terminates TLS for the host. Settings → World → room server overrides it.
pub const DEFAULT_SERVER: &str = "wss://chat.tonho.wtf/v1/room";
const VISITOR_ENT_BASE: u32 = 1000;

struct Session {
    role: &'static str,
    code: String,
    server: String,
    out: mpsc::Sender<Message>,
    cancel: CancellationToken,
    visitors: Vec<(u32, String)>,
    ent: Option<u32>,
    host_name: Option<String>,
}

static SESSION: Mutex<Option<Session>> = Mutex::new(None);

fn session() -> std::sync::MutexGuard<'static, Option<Session>> {
    SESSION.lock().unwrap_or_else(|e| e.into_inner())
}

fn state_json() -> Value {
    match session().as_ref() {
        Some(s) => json!({
            "role": s.role,
            "code": s.code,
            "server": s.server,
            "ent": s.ent,
            "host_name": s.host_name,
            "visitors": s.visitors.iter().map(|(id, name)| json!({ "visitor_id": id, "name": name })).collect::<Vec<_>>(),
        }),
        None => json!({ "role": "none", "visitors": [] }),
    }
}

fn emit_state(app: &AppHandle) {
    let _ = app.emit(EVENT_STATE, state_json());
}

/// `host`, `host:port`, `http(s)://…` or `ws(s)://…` → a `ws(s)://…/v1/room` URL.
pub fn normalize_server(raw: Option<String>, app: &AppHandle) -> String {
    let raw = raw
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            let saved = crate::storage::config::load_settings(app).world.room_server;
            (!saved.trim().is_empty()).then(|| saved.trim().to_string())
        })
        .unwrap_or_else(|| DEFAULT_SERVER.to_string());
    let with_scheme = if let Some(rest) = raw.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = raw.strip_prefix("http://") {
        format!("ws://{rest}")
    } else if raw.starts_with("ws://") || raw.starts_with("wss://") {
        raw
    } else {
        format!("ws://{raw}")
    };
    let after_scheme = with_scheme.split_once("://").map(|(_, r)| r).unwrap_or("");
    if after_scheme.trim_end_matches('/').contains('/') {
        with_scheme
    } else {
        format!("{}/v1/room", with_scheme.trim_end_matches('/'))
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn frame(op: u8, payload: &[u8]) -> Message {
    let mut bytes = Vec::with_capacity(payload.len() + 1);
    bytes.push(op);
    bytes.extend_from_slice(payload);
    Message::Binary(bytes.into())
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Connect, say hello, and read the one reply that decides everything.
async fn handshake(server: &str, hello: Value) -> Result<(Ws, Value), String> {
    let connect = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio_tungstenite::connect_async(server),
    );
    let (mut ws, _) = connect
        .await
        .map_err(|_| format!("{ERR_HOUSE}_UNREACHABLE: {server} did not answer"))?
        .map_err(|e| format!("{ERR_HOUSE}_UNREACHABLE: {e}"))?;
    ws.send(Message::Text(hello.to_string().into()))
        .await
        .map_err(|e| format!("{ERR_HOUSE}_UNREACHABLE: {e}"))?;
    let reply = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while let Some(msg) = ws.next().await {
            if let Ok(Message::Text(t)) = msg {
                return serde_json::from_str::<Value>(&t).ok();
            }
        }
        None
    })
    .await
    .ok()
    .flatten()
    .ok_or_else(|| format!("{ERR_HOUSE}_PROTOCOL: no answer to the hello"))?;
    if reply["op"] == "error" {
        return Err(format!(
            "{}: {}",
            reply["code"].as_str().unwrap_or("ERR_ROOM"),
            reply["message"].as_str().unwrap_or("refused")
        ));
    }
    Ok((ws, reply))
}

fn signed_hello(app: &AppHandle, op: &str, what: &str, extra: Value) -> Result<Value, String> {
    let profiles = app.state::<crate::AppState>().profile.clone();
    let profile = profiles.get()?;
    let ts = now_ms();
    let sig = profiles.sign(format!("omniworld:{what}:{ts}").as_bytes())?;
    let mut hello = json!({
        "op": op,
        "pubkey": hex::encode(profile.public_key),
        "name": profile.nickname,
        "ts": ts,
        "sig": hex::encode(sig),
    });
    if let (Some(h), Some(e)) = (hello.as_object_mut(), extra.as_object()) {
        h.extend(e.clone());
    }
    Ok(hello)
}

struct NetSink(mpsc::Sender<Message>);

impl DiffSink for NetSink {
    fn send(&self, bytes: Vec<u8>) -> Result<(), String> {
        match self.0.try_send(frame(OP_WORLD_DIFF, &bytes)) {
            // A full queue drops this blob; the visitors catch up on the next
            // resync. A closed one ends the house.
            Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => Ok(()),
            Err(mpsc::error::TrySendError::Closed(_)) => Err("house closed".into()),
        }
    }
}

fn spawn_tile(manager: &crate::world_manager::WorldManager) -> omniget_world::map::Tile {
    manager
        .map_def()
        .and_then(|m| m.markers.iter().find(|k| k.name == "spawn").map(|k| k.tile))
        .unwrap_or(omniget_world::map::Tile::new(1, 1))
}

fn world_input(manager: &crate::world_manager::WorldManager, input: omniget_world::world::Input) {
    manager.submit(super::input::BridgeInput::World(Box::new(input)));
}

fn end(app: &AppHandle, reason: &str) {
    let ended = session().take();
    if let Some(s) = ended {
        s.cancel.cancel();
        if s.role == "host" {
            if let Some(manager) = super::session::manager(&app.state::<crate::AppState>()) {
                manager.set_net_sink(None);
                for (id, _) in &s.visitors {
                    world_input(
                        &manager,
                        omniget_world::world::Input::Despawn {
                            ent: omniget_world::ents::EntId(VISITOR_ENT_BASE + id),
                        },
                    );
                }
            }
        }
        let _ = app.emit(EVENT_CLOSED, json!({ "reason": reason, "role": s.role }));
    }
    emit_state(app);
}

/// One loop per session: drains the outgoing queue and reads the socket.
fn run(
    app: AppHandle,
    mut ws: Ws,
    mut out_rx: mpsc::Receiver<Message>,
    cancel: CancellationToken,
    channel: Option<Channel<InvokeResponseBody>>,
) {
    tauri::async_runtime::spawn(async move {
        let reason = loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    let _ = ws.send(Message::Close(None)).await;
                    break "left";
                }
                Some(msg) = out_rx.recv() => {
                    if ws.send(msg).await.is_err() {
                        break "connection_lost";
                    }
                }
                incoming = ws.next() => {
                    match incoming {
                        Some(Ok(Message::Binary(bytes))) if !bytes.is_empty() => on_binary(&app, bytes[0], &bytes[1..], channel.as_ref()),
                        Some(Ok(Message::Text(text))) => {
                            if let Ok(v) = serde_json::from_str::<Value>(&text) {
                                if v["op"] == "closed" {
                                    break if v["reason"] == "ttl" { "ttl" } else { "host_left" };
                                }
                                on_control(&app, &v);
                            }
                        }
                        Some(Ok(Message::Ping(p))) => {
                            let _ = ws.send(Message::Pong(p)).await;
                        }
                        Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break "connection_lost",
                        _ => {}
                    }
                }
            }
        };
        end(&app, reason);
    });
}

fn on_control(app: &AppHandle, v: &Value) {
    let id = v["visitor_id"].as_u64().unwrap_or(0) as u32;
    let is_host = session()
        .as_ref()
        .map(|s| s.role == "host")
        .unwrap_or(false);
    match v["op"].as_str().unwrap_or("") {
        "visitor_joined" => {
            let name = v["name"].as_str().unwrap_or("guest").to_string();
            if let Some(s) = session().as_mut() {
                s.visitors.retain(|(i, _)| *i != id);
                s.visitors.push((id, name.clone()));
            }
            if is_host {
                if let Some(manager) = super::session::manager(&app.state::<crate::AppState>()) {
                    world_input(
                        &manager,
                        omniget_world::world::Input::Spawn {
                            ent: omniget_world::ents::EntId(VISITOR_ENT_BASE + id),
                            name,
                            at: spawn_tile(&manager),
                        },
                    );
                    // The newcomer has no picture yet: send everyone a whole one.
                    manager.resync();
                }
            }
        }
        "visitor_left" => {
            if let Some(s) = session().as_mut() {
                s.visitors.retain(|(i, _)| *i != id);
            }
            if is_host {
                if let Some(manager) = super::session::manager(&app.state::<crate::AppState>()) {
                    world_input(
                        &manager,
                        omniget_world::world::Input::Despawn {
                            ent: omniget_world::ents::EntId(VISITOR_ENT_BASE + id),
                        },
                    );
                }
            }
        }
        _ => return,
    }
    emit_state(app);
}

fn on_binary(
    app: &AppHandle,
    op: u8,
    payload: &[u8],
    channel: Option<&Channel<InvokeResponseBody>>,
) {
    match op {
        OP_WORLD_DIFF => {
            if let Some(ch) = channel {
                let _ = ch.send(InvokeResponseBody::Raw(payload.to_vec()));
            }
        }
        OP_CHAT => {
            if let Ok(v) = serde_json::from_slice::<Value>(payload) {
                let _ = app.emit(EVENT_CHAT, v);
            }
        }
        // Host side: a visitor lost a diff (a full queue drops blobs) and asks
        // for a whole picture. At most one resync a second, whoever asks.
        OP_INTEREST => {
            let is_host = session()
                .as_ref()
                .map(|s| s.role == "host")
                .unwrap_or(false);
            static LAST: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
            let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
            let due = last
                .map(|t| t.elapsed().as_millis() >= 1000)
                .unwrap_or(true);
            if is_host && due {
                *last = Some(std::time::Instant::now());
                if let Some(manager) = super::session::manager(&app.state::<crate::AppState>()) {
                    manager.resync();
                }
            }
        }
        // Host side: a visitor walks. Nothing else is accepted from the network.
        OP_WORLD_INPUT if payload.len() > 4 => {
            let id = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
            let known = session()
                .as_ref()
                .map(|s| s.role == "host" && s.visitors.iter().any(|(i, _)| *i == id))
                .unwrap_or(false);
            let Ok(v) = serde_json::from_slice::<Value>(&payload[4..]) else {
                return;
            };
            if !known || v["type"] != "move" {
                return;
            }
            let (Some(x), Some(y)) = (v["to"][0].as_i64(), v["to"][1].as_i64()) else {
                return;
            };
            if let Some(manager) = super::session::manager(&app.state::<crate::AppState>()) {
                world_input(
                    &manager,
                    omniget_world::world::Input::Move {
                        ent: omniget_world::ents::EntId(VISITOR_ENT_BASE + id),
                        to: omniget_world::map::Tile::new(x as i32, y as i32),
                    },
                );
            }
        }
        _ => {}
    }
}

#[tauri::command]
pub async fn house_status() -> Result<Value, String> {
    Ok(state_json())
}

/// Opens this machine's house on the room server and returns the code.
#[tauri::command]
pub async fn house_open(
    app: AppHandle,
    state: tauri::State<'_, crate::AppState>,
    server: Option<String>,
) -> Result<Value, String> {
    if session().is_some() {
        return Err(format!(
            "{ERR_HOUSE}_BUSY: close the current house or visit first"
        ));
    }
    let manager = super::session::ensure_manager(&app, &state)?;
    if !manager.has_world() {
        return Err(super::ERR_NO_WORLD.to_string());
    }
    let server = normalize_server(server, &app);
    let hello = signed_hello(&app, "open", "open", json!({ "house": manager.house() }))?;
    let (ws, reply) = handshake(&server, hello).await?;
    let code = reply["code"]
        .as_str()
        .ok_or_else(|| format!("{ERR_HOUSE}_PROTOCOL: no code"))?
        .to_string();

    let (out, out_rx) = mpsc::channel::<Message>(512);
    let cancel = CancellationToken::new();
    *session() = Some(Session {
        role: "host",
        code: code.clone(),
        server: server.clone(),
        out: out.clone(),
        cancel: cancel.clone(),
        visitors: Vec::new(),
        ent: None,
        host_name: None,
    });
    manager.set_net_sink(Some(Arc::new(NetSink(out))));
    run(app.clone(), ws, out_rx, cancel, None);
    emit_state(&app);
    Ok(state_json())
}

#[tauri::command]
pub async fn house_close(app: AppHandle) -> Result<Value, String> {
    end(&app, "left");
    Ok(state_json())
}

/// Visits a house. Answers with the first whole snapshot, like `world_open`;
/// everything after it comes through `channel`.
#[tauri::command]
pub async fn house_join(
    app: AppHandle,
    code: String,
    server: Option<String>,
    channel: Channel<InvokeResponseBody>,
) -> Result<Response, String> {
    if session().is_some() {
        return Err(format!(
            "{ERR_HOUSE}_BUSY: close the current house or visit first"
        ));
    }
    let code = code.trim().to_uppercase();
    let server = normalize_server(server, &app);
    let hello = signed_hello(
        &app,
        "join",
        &format!("join:{code}"),
        json!({ "code": code }),
    )?;
    let (mut ws, reply) = handshake(&server, hello).await?;

    // The host answers a join with a whole snapshot: wait for it here so the
    // canvas starts from a picture, not from a diff it cannot apply.
    let first = tokio::time::timeout(std::time::Duration::from_secs(20), async {
        while let Some(msg) = ws.next().await {
            match msg {
                Ok(Message::Binary(b)) if b.first() == Some(&OP_WORLD_DIFF) => {
                    return Some(b[1..].to_vec())
                }
                Ok(Message::Ping(p)) => {
                    let _ = ws.send(Message::Pong(p)).await;
                }
                Ok(Message::Close(_)) | Err(_) => return None,
                _ => {}
            }
        }
        None
    })
    .await
    .ok()
    .flatten()
    .ok_or_else(|| {
        format!("{ERR_HOUSE}_NO_SNAPSHOT: the host sent no world (is their /world tab open?)")
    })?;

    let (out, out_rx) = mpsc::channel::<Message>(128);
    let cancel = CancellationToken::new();
    *session() = Some(Session {
        role: "visitor",
        code,
        server,
        out,
        cancel: cancel.clone(),
        visitors: reply["visitors"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|v| {
                (
                    v["visitor_id"].as_u64().unwrap_or(0) as u32,
                    v["name"].as_str().unwrap_or("guest").to_string(),
                )
            })
            .collect(),
        ent: reply["ent"].as_u64().map(|e| e as u32),
        host_name: reply["host_name"].as_str().map(str::to_string),
    });
    run(app.clone(), ws, out_rx, cancel, Some(channel));
    emit_state(&app);
    Ok(Response::new(first))
}

#[tauri::command]
pub async fn house_leave(app: AppHandle) -> Result<Value, String> {
    end(&app, "left");
    Ok(state_json())
}

fn send(msg: Message) -> Result<(), String> {
    let out = session()
        .as_ref()
        .map(|s| s.out.clone())
        .ok_or_else(|| format!("{ERR_HOUSE}_CLOSED: not in a house"))?;
    out.try_send(msg)
        .map_err(|_| format!("{ERR_HOUSE}_BUSY: the connection is saturated"))
}

/// The visitor's own input. Only `{ "type": "move", "to": [x, y] }` means
/// anything to the host.
#[tauri::command]
pub async fn house_input(input: Value) -> Result<(), String> {
    let op = if input["type"] == "interest" {
        OP_INTEREST
    } else {
        OP_WORLD_INPUT
    };
    send(frame(op, input.to_string().as_bytes()))
}

#[tauri::command]
pub async fn house_chat(text: String) -> Result<(), String> {
    let text: String = text
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(500)
        .collect();
    if text.is_empty() {
        return Ok(());
    }
    send(frame(OP_CHAT, text.as_bytes()))
}
