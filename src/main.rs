// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 ZT-DSP Suite contributors

//! ZT-DSP 1.0.1 服务端：登记 → 配对 → 中继，全程不解密。

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

/// 同时挂起等待的「被拒连接」上限。名额耗尽后直接关闭连接，
/// 避免洪泛时后台任务数无界增长。
const MAX_PENDING_DRAINS: usize = 256;

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
#[command(name = "zt-dsp-server", version, about = "ZT-DSP 1.0.1 server")]
struct Args {
    #[arg(long)]
    listen: Option<SocketAddr>,
    #[arg(long)]
    control: Option<SocketAddr>,
    #[arg(long, value_enum, default_value = "beginner")]
    profile: ProfileArg,
    #[arg(long)]
    config: Option<PathBuf>,
    #[arg(long, env = "ZT_DSP_CONTROL_TOKEN")]
    control_token: Option<String>,
    #[arg(long)]
    dump_config: bool,
}

struct Shared {
    cfg: Config,
    lobby: Lobby<TcpStream>,
    metrics: Arc<Metrics>,
    offline: Arc<OfflineStore>,
    directory: Arc<Directory>,
    per_ip: Mutex<HashMap<IpAddr, usize>>,
    /// 限制「拒绝后挂起」这一类后台任务的数量：没有它，洪泛时每个被拒连接
    /// 都会 spawn 一个 15 秒的 drain 任务，并发任务数不受 max_connections 约束。
    drain_limit: Arc<tokio::sync::Semaphore>,
}

struct ConnGuard(Arc<Metrics>);

impl Drop for ConnGuard {
    fn drop(&mut self) {
        self.0.active_conns.fetch_sub(1, Ordering::Relaxed);
    }
}

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
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let args = Args::parse();

    let mut cfg = match &args.config {
        Some(p) => Config::load(p)?,
        None => Config::for_profile(args.profile.into()),
    };
    if let Some(l) = args.listen {
        cfg.listen = l;
    }
    if let Some(c) = args.control {
        cfg.control_listen = Some(c);
    }

    if args.dump_config {
        println!("{}", serde_json::to_string_pretty(&cfg)?);
        return Ok(());
    }

    if cfg.control_listen.is_some() && args.control_token.is_none() {
        return Err("control plane requires --control-token".into());
    }

    eprintln!("ZT-DSP · ZT-DSP_SV-{}", env!("CARGO_PKG_VERSION"));
    info!(
        profile = cfg.profile.as_str(),
        listen = %cfg.listen,
        control = ?cfg.control_listen,
        directory = cfg.directory.enabled,
        "starting ZT-DSP relay (no decryption)"
    );
    if cfg.directory.enabled {
        warn!("identity directory enabled: deviates from spec line 979");
    }

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
        drain_limit: Arc::new(tokio::sync::Semaphore::new(MAX_PENDING_DRAINS)),
    });

    if let Some(caddr) = cfg.control_listen {
        let token = args.control_token.clone().expect("checked above");
        let state = Arc::new(ControlState {
            offline: Arc::clone(&offline),
            groups: Arc::clone(&groups),
            directory: Arc::clone(&directory),
            metrics: Arc::clone(&metrics),
            token,
        });
        tokio::spawn(async move {
            if let Err(err) = control::serve(caddr, state).await {
                warn!(error = %err, "control plane failed");
            }
        });
    }

    {
        let shared = Arc::clone(&shared);
        tokio::spawn(async move {
            let mut lobby_tick = tokio::time::interval(Duration::from_secs(5));
            let mut offline_tick = tokio::time::interval(Duration::from_secs(60));
            let mut dir_tick = tokio::time::interval(Duration::from_secs(60));
            loop {
                tokio::select! {
                    _ = lobby_tick.tick() => {
                        let n = shared.lobby.sweep();
                        if n > 0 { debug!(n, "lobby sweep"); }
                    }
                    _ = offline_tick.tick() => {
                        let n = shared.offline.sweep();
                        if n > 0 { debug!(n, "offline sweep"); }
                    }
                    _ = dir_tick.tick() => {
                        let n = shared.directory.sweep();
                        if n > 0 { debug!(n, "directory sweep"); }
                    }
                }
            }
        });
    }

    let listener = TcpListener::bind(cfg.listen).await?;
    info!(addr = %cfg.listen, "data plane listening");

    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(v) => v,
            Err(err) => {
                warn!(error = %err, "accept failed");
                continue;
            }
        };

        if shared.metrics.active_conns.load(Ordering::Relaxed) as usize
            >= shared.cfg.max_connections
        {
            Metrics::inc(&shared.metrics.rejected_silent);
            // 过载也不得直接 FIN：与其它拒绝路径保持同样的静默观感与时长。
            // 但挂起任务数必须有上限，否则洪泛时任务数会失控。
            let d = Duration::from_secs(shared.cfg.handshake_timeout_secs.max(1));
            if let Ok(permit) = Arc::clone(&shared.drain_limit).try_acquire_owned() {
                let mut s = stream;
                tokio::spawn(async move {
                    let _permit = permit;
                    let _ = silent::drain_silently(&mut s, d).await;
                });
            } else {
                // 挂起名额耗尽：宁可提前关闭，也不让任务数无界增长
                drop(stream);
            }
            continue;
        }
        shared.metrics.active_conns.fetch_add(1, Ordering::Relaxed);
        Metrics::inc(&shared.metrics.accepted);

        let shared2 = Arc::clone(&shared);
        tokio::spawn(async move {
            let _g = ConnGuard(Arc::clone(&shared2.metrics));
            handle_conn(stream, peer, shared2).await;
        });
    }
}

async fn handle_conn(mut stream: TcpStream, peer: SocketAddr, shared: Arc<Shared>) {
    stream.set_nodelay(true).ok();

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
        drain_like_handshake(&mut stream, &shared).await;
        return;
    }
    let ip_guard = IpGuard {
        map: &shared.per_ip,
        ip,
    };

    let deadline = Instant::now() + Duration::from_secs(shared.cfg.handshake_timeout_secs.max(1));

    let my_frame = match read_phase1(&mut stream, deadline).await {
        Ok(f) => f,
        Err(err) => {
            Metrics::inc(&shared.metrics.rejected_silent);
            debug!(peer = %peer, error = %err, "phase1 rejected");
            drain_like_handshake(&mut stream, &shared).await;
            drop(ip_guard);
            return;
        }
    };
    debug!(
        peer = %peer,
        frame_len = my_frame.raw_len(),
        aead = %my_frame.param.aead,
        "phase1 accepted"
    );
    let identity = my_frame.identity;

    match shared.lobby.register(my_frame.clone()) {
        Outcome::Matched {
            peer_frame: _,
            deliver_to_peer,
        } => {
            let handed = deliver_to_peer.send(Handoff {
                frame: my_frame,
                payload: stream,
            });
            if handed.is_err() {
                debug!(peer = %peer, "initiator already gone");
            }
            drop(ip_guard);
        }
        Outcome::Waiting { rx, ticket } => {
            let lobby_timeout = Duration::from_secs(shared.cfg.lobby_timeout_secs.max(1));
            match timeout(lobby_timeout, rx).await {
                Ok(Ok(handoff)) => {
                    Metrics::inc(&shared.metrics.paired);
                    shared.metrics.active_pairs.fetch_add(1, Ordering::Relaxed);
                    if shared.directory.enabled() {
                        shared.directory.touch(identity.src);
                        shared.directory.touch(identity.dst);
                    }

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
                        write_timeout: Some(Duration::from_secs(30)),
                    };
                    match relay_pair(stream, peer_stream, opts, &shared.metrics).await {
                        Ok((ab, ba, cause)) => {
                            debug!(peer = %peer, ab, ba, ?cause, "relay finished")
                        }
                        Err(err) => debug!(peer = %peer, error = %err, "relay failed"),
                    }
                    shared.metrics.active_pairs.fetch_sub(1, Ordering::Relaxed);
                    drop(ip_guard);
                }
                _ => {
                    shared.lobby.withdraw(ticket);
                    Metrics::inc(&shared.metrics.lobby_timeout);
                    debug!(peer = %peer, "lobby timeout");
                    drain_like_handshake(&mut stream, &shared).await;
                    drop(ip_guard);
                }
            }
        }
    }
}

async fn drain_like_handshake(stream: &mut TcpStream, shared: &Shared) {
    let _ = silent::drain_silently(
        stream,
        Duration::from_secs(shared.cfg.handshake_timeout_secs.max(1)),
    )
    .await;
}

async fn write_frame(s: &mut TcpStream, f: &Phase1Frame) -> std::io::Result<()> {
    s.write_all(&f.raw).await?;
    s.flush().await
}
