// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 ZT-DSP Suite contributors

//! L4 控制面：JSON Lines over TCP。
//!
//! **与数据面完全隔离** —— 它不碰 ZT-DSP 线上帧，因此不影响与现有客户端的互操作。
//! 默认只绑 127.0.0.1；若配置了 token，第一条消息必须先鉴权。
//!
//! 所有 payload 都是**不透明十六进制字节**：服务器不解密、不解析。

use std::net::SocketAddr;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tracing::{debug, info, warn};

use crate::l4::{Directory, Groups, L4Error, OfflineStore};
use crate::metrics::Metrics;
use crate::util::{from_hex, parse_token_hash, to_hex};

/// 控制面共享状态。
pub struct ControlState {
    pub offline: Arc<OfflineStore>,
    pub groups: Arc<Groups>,
    pub directory: Arc<Directory>,
    pub metrics: Arc<Metrics>,
    pub token: Option<String>,
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Req {
    Auth { token: String },
    Ping,
    OfflinePush { dst: String, payload: String },
    OfflinePull { dst: String, max: Option<usize> },
    GroupCreate { name: String, owner: String },
    GroupJoin { name: String, member: String },
    GroupSend { name: String, sender: String, payload: String },
    GroupPull { name: String, member: String },
    DirLookup { hash: String },
    Stats,
}

#[derive(Serialize)]
struct Resp {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<serde_json::Value>,
}

impl Resp {
    fn ok(data: serde_json::Value) -> Self {
        Self { ok: true, error: None, data: Some(data) }
    }
    fn err(e: impl std::fmt::Display) -> Self {
        Self { ok: false, error: Some(e.to_string()), data: None }
    }
}

fn e(e: L4Error) -> Resp {
    Resp::err(e)
}

/// 启动控制面。
pub async fn serve(addr: SocketAddr, state: Arc<ControlState>) -> std::io::Result<()> {
    let listener = TcpListener::bind(addr).await?;
    info!(%addr, "控制面已监听");
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(v) => v,
            Err(err) => {
                warn!(error = %err, "控制面 accept 失败");
                continue;
            }
        };
        let st = Arc::clone(&state);
        tokio::spawn(async move {
            if let Err(err) = handle(stream, st).await {
                debug!(%peer, error = %err, "控制面连接结束");
            }
        });
    }
}

async fn handle(stream: TcpStream, state: Arc<ControlState>) -> std::io::Result<()> {
    stream.set_nodelay(true).ok();
    let (rd, mut wr) = stream.into_split();
    let mut lines = BufReader::new(rd).lines();

    let mut authed = state.token.is_none();

    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let resp = match serde_json::from_str::<Req>(&line) {
            Err(err) => Resp::err(format!("请求解析失败: {err}")),
            Ok(Req::Auth { token }) => {
                if state.token.as_deref() == Some(token.as_str()) {
                    authed = true;
                    Resp::ok(serde_json::json!({"authed": true}))
                } else {
                    Resp::err("鉴权失败")
                }
            }
            Ok(_) if !authed => Resp::err("未鉴权：请先发送 {\"op\":\"auth\",\"token\":\"…\"}"),
            Ok(req) => dispatch(req, &state),
        };
        let mut out = serde_json::to_vec(&resp).unwrap_or_else(|_| b"{\"ok\":false}".to_vec());
        out.push(b'\n');
        wr.write_all(&out).await?;
        wr.flush().await?;
    }
    Ok(())
}

fn dispatch(req: Req, state: &ControlState) -> Resp {
    match req {
        Req::Auth { .. } => Resp::ok(serde_json::json!({"authed": true})),
        Req::Ping => Resp::ok(serde_json::json!({"pong": true})),

        Req::OfflinePush { dst, payload } => {
            let Some(d) = parse_token_hash(&dst) else {
                return Resp::err("dst 必须是 64 个十六进制字符");
            };
            let Some(bytes) = from_hex(&payload) else {
                return Resp::err("payload 必须是十六进制");
            };
            match state.offline.push(d, bytes) {
                Ok(depth) => {
                    Metrics::inc(&state.metrics.offline_stored);
                    Resp::ok(serde_json::json!({"depth": depth}))
                }
                Err(err) => e(err),
            }
        }

        Req::OfflinePull { dst, max } => {
            let Some(d) = parse_token_hash(&dst) else {
                return Resp::err("dst 必须是 64 个十六进制字符");
            };
            match state.offline.pull(d, max.unwrap_or(100)) {
                Ok(items) => {
                    Metrics::add(&state.metrics.offline_delivered, items.len() as u64);
                    let hexed: Vec<String> = items.iter().map(|v| to_hex(v)).collect();
                    Resp::ok(serde_json::json!({"items": hexed}))
                }
                Err(err) => e(err),
            }
        }

        Req::GroupCreate { name, owner } => {
            let Some(o) = parse_token_hash(&owner) else {
                return Resp::err("owner 必须是 64 个十六进制字符");
            };
            match state.groups.create(&name, o) {
                Ok(()) => Resp::ok(serde_json::json!({"created": name})),
                Err(err) => e(err),
            }
        }

        Req::GroupJoin { name, member } => {
            let Some(m) = parse_token_hash(&member) else {
                return Resp::err("member 必须是 64 个十六进制字符");
            };
            match state.groups.join(&name, m) {
                Ok(n) => Resp::ok(serde_json::json!({"members": n})),
                Err(err) => e(err),
            }
        }

        Req::GroupSend { name, sender, payload } => {
            let Some(s) = parse_token_hash(&sender) else {
                return Resp::err("sender 必须是 64 个十六进制字符");
            };
            let Some(bytes) = from_hex(&payload) else {
                return Resp::err("payload 必须是十六进制");
            };
            match state.groups.send(&name, s, bytes) {
                Ok(cursor) => {
                    Metrics::inc(&state.metrics.group_broadcasts);
                    Resp::ok(serde_json::json!({"cursor": cursor}))
                }
                Err(err) => e(err),
            }
        }

        Req::GroupPull { name, member } => {
            let Some(m) = parse_token_hash(&member) else {
                return Resp::err("member 必须是 64 个十六进制字符");
            };
            match state.groups.pull(&name, m) {
                Ok(items) => {
                    let out: Vec<serde_json::Value> = items
                        .into_iter()
                        .map(|(c, p)| serde_json::json!({"cursor": c, "payload": to_hex(&p)}))
                        .collect();
                    Resp::ok(serde_json::json!({"items": out}))
                }
                Err(err) => e(err),
            }
        }

        Req::DirLookup { hash } => {
            let Some(h) = parse_token_hash(&hash) else {
                return Resp::err("hash 必须是 64 个十六进制字符");
            };
            match state.directory.lookup(h, state.directory.allow_arbitrary()) {
                Ok((alive, ago)) => Resp::ok(serde_json::json!({"alive": alive, "ago_secs": ago})),
                Err(err) => e(err),
            }
        }

        Req::Stats => Resp::ok(serde_json::json!({
            "accepted": Metrics::get(&state.metrics.accepted),
            "paired": Metrics::get(&state.metrics.paired),
            "active_pairs": Metrics::get(&state.metrics.active_pairs),
            "rejected_silent": Metrics::get(&state.metrics.rejected_silent),
            "lobby_timeout": Metrics::get(&state.metrics.lobby_timeout),
            "bytes_in": Metrics::get(&state.metrics.bytes_in),
            "bytes_out": Metrics::get(&state.metrics.bytes_out),
            "offline_depth": state.offline.depth(),
            "offline_stored": Metrics::get(&state.metrics.offline_stored),
            "group_broadcasts": Metrics::get(&state.metrics.group_broadcasts),
        })),
    }
}
