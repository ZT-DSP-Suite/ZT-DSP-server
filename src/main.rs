// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 ZT-DSP Suite contributors

//! ZT-DSP 1.0.1 服务端。
//!
//! 酒店前台模型：登记 → 配对 → 转发，**全程不解密**。
//! 数据面严格遵循附录 F；L4 扩展走独立控制面，不污染线上协议。

mod config;
mod control;
mod frame;
mod l4;
mod lobby;
mod metrics;
mod relay;
mod silent;
mod util;

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use clap::{Parser, ValueEnum};
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::time::{timeout, Instant};
use tracing::{debug, info, warn};
use tracing_subscriber::EnvFilter;

use crate::config::{Config, Profile};
use crate::control::ControlState;
use crate::frame::{read_phase1, Phase1Frame};
use crate::l4::{Directory, Groups, OfflineStore};
use crate::lobby::{Handoff, Lobby, Outcome};
use crate::metrics::Metrics;
use crate::relay::{relay_pair, RelayOpts};

#[derive(Copy, Clone, Debug, ValueEnum)]
enum ProfileArg {
    Beginner,
    Advanced,
    HighAnonymity,
}

impl From<ProfileArg> for Profile {
    fn from(p: ProfileArg) -> Self {
        match p {
            ProfileArg::Beginner => Profile::Beginner,
            ProfileArg::Advanced => Profile::Advanced,
            ProfileArg::HighAnonymity => Profile::HighAnonymity,
        }
    }
}

#[derive(Parser, Debug)]
#[command(name = "zt-dsp-server", version, about = "ZT-DSP 1.0.1 服务端（配对 + 中继，不解密）")]
struct Args {
    /// 数据面监听地址
    #[arg(long, default_value = "0.0.0.0:7443")]
    listen: SocketAddr,
    /// 控制面监听地址（L4 扩展）；不给则关闭
    #[arg(long)]
    control: Option<SocketAddr>,
    /// 配置档位
    #[arg(long, value_enum, default_value = "beginner")]
    profile: ProfileArg,
    /// JSON 配置文件（覆盖档位默认值）
    #[arg(long)]
    config: Option<PathBuf>,
    /// 控制面鉴权口令；只对 127.0.0.1 绑定时可不设
    #[arg(long, env = "ZT_DSP_CONTROL_TOKEN")]
    control_token: Option<String>,
    /// 打印最终配置后退出
    #[arg(long)]
    dump_config: bool,
}

/// 全进程共享状态。
struct Shared {
    cfg: Config,
    lobby: Lobby<TcpStream>,
    metrics: Arc<Metrics>,
    offline: Arc<OfflineStore>,
    directory: Arc<Directory>,
    per_ip: Mutex<HashMap<IpAddr, usize>>,
}

/// 单 IP 计数守卫：连接结束自动减一。
struct IpGuard<'a> {
    map: &'a Mutex<HashMap<IpAddr, usize>>,
    ip: IpAddr,
}

impl Drop for IpGuard<'_> {
    fn drop(&mut self) {
        let mut g = self.map.lock().expect("per_ip mutex poisoned");
        if let Some(n) = g.get_mut(&self.ip) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                g.remove(&self.ip);
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with_writer(std::io::stderr)
        .init();

    let args = Args::parse();

    let mut cfg = match &args.config {
        Some(p) => Config::load(p)?,
        None => Config::for_profile(args.profile.into()),
    };
    cfg.listen = args.listen;
    if args.control.is_some() {
        cfg.control_listen = args.control;
    }

    if args.dump_config {
        println!("{}", serde_json::to_string_pretty(&cfg)?);
        return Ok(());
    }

    info!(
        profile = cfg.profile.as_str(),
        listen = %cfg.listen,
        "启动 ZT-DSP 服务端（纯字节中继，不解密）"
    );

    let metrics = Arc::new(Metrics::default());
    let offline = Arc::new(OfflineStore::new(cfg.offline.clone()));
    let groups = Arc::new(Groups::new(cfg.group.clone()));
    let directory = Arc::new(Directory::new(cfg.directory.clone()));

    let shared = Arc::new(Shared {
        cfg: cfg.clone(),
        lobby: Lobby::new(),
        metrics: Arc::clone(&metrics),
        offline: Arc::clone(&offline),
        directory: Arc::clone(&directory),
        per_ip: Mutex::new(HashMap::new()),
    });

    if let Some(caddr) = cfg.control_listen {
        if caddr.ip().is_loopback() || args.control_token.is_some() {
            let state = Arc::new(ControlState {
                offline: Arc::clone(&offline),
                groups: Arc::clone(&groups),
                directory: Arc::clone(&directory),
                metrics: Arc::clone(&metrics),
                token: args.control_token.clone(),
            });
            tokio::spawn(async move {
                if let Err(err) = control::serve(caddr, state).await {
                    warn!(error = %err, "控制面启动失败");
                }
            });
        } else {
            warn!("控制面绑定在非本地地址却没有 --control-token，已拒绝启动（防止未授权访问）");
        }
    }

    {
        let shared = Arc::clone(&shared);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(5));
            loop {
                tick.tick().await;
                let swept = shared.lobby.sweep();
                if swept > 0 {
                    debug!(swept, "清理等候池僵尸登记");
                }
                let off = shared.offline.sweep();
                if off > 0 {
                    debug!(off, "清理过期离线消息");
                }
                shared.directory.sweep(Duration::from_secs(86_400));
            }
        });
    }

    let listener = TcpListener::bind(cfg.listen).await?;
    info!(addr = %cfg.listen, "数据面已监听");

    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(v) => v,
            Err(err) => {
                warn!(error = %err, "accept 失败");
                continue;
            }
        };
        let shared = Arc::clone(&shared);
        tokio::spawn(async move {
            handle_conn(stream, peer, shared).await;
        });
    }
}

/// 连接计数守卫：进入时加一，离开时减一。覆盖**未配对**的连接，
/// 因此洪泛连接同样计入全局上限。
struct ConnGuard<'a>(&'a Metrics);

impl Drop for ConnGuard<'_> {
    fn drop(&mut self) {
        self.0.active_conns.fetch_sub(1, Ordering::Relaxed);
    }
}

async fn handle_conn(mut stream: TcpStream, peer: SocketAddr, shared: Arc<Shared>) {
    let _conn_guard = ConnGuard(&shared.metrics);
    let conns = shared.metrics.active_conns.fetch_add(1, Ordering::Relaxed);
    Metrics::inc(&shared.metrics.accepted);
    stream.set_nodelay(true).ok();

    if conns as usize >= shared.cfg.max_connections {
        Metrics::inc(&shared.metrics.rejected_silent);
        let _ = silent::drain_silently(&mut stream, Duration::from_secs(1)).await;
        return;
    }

    let ip = peer.ip();
    let denied = {
        let limit = shared.cfg.max_per_ip;
        let mut map = shared.per_ip.lock().expect("per_ip mutex poisoned");
        let entry = map.entry(ip).or_insert(0);
        if limit > 0 && *entry >= limit {
            true
        } else {
            *entry += 1;
            false
        }
    };
    if denied {
        Metrics::inc(&shared.metrics.rejected_silent);
        let _ = silent::drain_silently(&mut stream, Duration::from_secs(1)).await;
        return;
    }
    let ip_guard = IpGuard { map: &shared.per_ip, ip };

    let deadline = Instant::now() + Duration::from_secs(shared.cfg.handshake_timeout_secs.max(1));

    let my_frame = match read_phase1(&mut stream, deadline).await {
        Ok(f) => f,
        Err(err) => {
            Metrics::inc(&shared.metrics.rejected_silent);
            debug!(peer = %peer, error = %err, "阶段一读取失败（静默丢弃，对端无感）");
            // 与合法握手失败的对端等待同样的窗口，避免时序可区分
            let _ = silent::drain_silently(
                &mut stream,
                Duration::from_secs(shared.cfg.handshake_timeout_secs.max(1)),
            )
            .await;
            drop(ip_guard);
            return;
        }
    };

    let identity = my_frame.identity;

    match shared.lobby.register(my_frame.clone()) {
        Outcome::Matched { peer_frame: _, deliver_to_peer } => {
            Metrics::inc(&shared.metrics.paired);
            shared.metrics.active_pairs.fetch_add(1, Ordering::Relaxed);
            if let Err(back) = deliver_to_peer.send(Handoff { frame: my_frame, payload: stream }) {
                debug!(peer = %peer, "配对对手已超时退出，丢弃本次连接");
                drop(back);
                shared.metrics.active_pairs.fetch_sub(1, Ordering::Relaxed);
            }
            drop(ip_guard);
            return;
        }
        Outcome::Waiting { rx, ticket } => {
            let lobby_timeout = Duration::from_secs(shared.cfg.lobby_timeout_secs.max(1));
            match timeout(lobby_timeout, rx).await {
                Ok(Ok(handoff)) => {
                    Metrics::inc(&shared.metrics.paired);
                    shared.metrics.active_pairs.fetch_add(1, Ordering::Relaxed);
                    shared.directory.touch(identity.dst);

                    let mut peer_stream = handoff.payload;
                    let written = async {
                        write_frame(&mut peer_stream, &my_frame).await?;
                        write_frame(&mut stream, &handoff.frame).await?;
                        Ok::<(), std::io::Error>(())
                    }
                    .await;

                    if written.is_err() {
                        shared.metrics.active_pairs.fetch_sub(1, Ordering::Relaxed);
                        Metrics::inc(&shared.metrics.rejected_silent);
                        drop(ip_guard);
                        return;
                    }

                    let opts = RelayOpts {
                        buffer_bytes: shared.cfg.buffer_bytes,
                        idle_timeout: if shared.cfg.idle_timeout_secs > 0 {
                            Some(Duration::from_secs(shared.cfg.idle_timeout_secs))
                        } else {
                            None
                        },
                    };
                    match relay_pair(stream, peer_stream, opts, &shared.metrics).await {
                        Ok((ab, ba)) => debug!(peer = %peer, ab, ba, "中继结束"),
                        Err(err) => debug!(peer = %peer, error = %err, "中继异常结束"),
                    }
                    shared.metrics.active_pairs.fetch_sub(1, Ordering::Relaxed);
                    drop(ip_guard);
                }
                _ => {
                    shared.lobby.withdraw(ticket);
                    Metrics::inc(&shared.metrics.lobby_timeout);
                    debug!(peer = %peer, "等候池超时");
                    let _ = silent::drain_silently(&mut stream, Duration::from_secs(1)).await;
                    drop(ip_guard);
                }
            }
        }
    }
}

async fn write_frame(s: &mut TcpStream, f: &Phase1Frame) -> std::io::Result<()> {
    s.write_all(&f.raw).await?;
    s.flush().await
}
