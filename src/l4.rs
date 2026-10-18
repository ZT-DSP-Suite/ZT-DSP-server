// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 ZT-DSP Suite contributors

//! L4 扩展：离线消息 / 群组 / 身份目录。
//!
//! ## 为什么不塞进 ZT-DSP 线上帧
//!
//! 附录 F 的线上帧**没有帧类型字段** —— 帧的含义由它在握手流程中的位置决定。
//! 往数据面插新帧会立刻踩两条红线：
//!   1. 破坏与现有客户端（zt_dsp_go / zt_dsp_rs / zt_dsp_py）的互操作；
//!   2. 违反严格静默丢弃契约（对端遇到未知帧只能静默丢弃）。
//!
//! 所以 L4 一律走**独立控制面**（JSON Lines over TCP，默认只绑 127.0.0.1），
//! 数据面保持 100% 规范一致、纯字节中继、服务器不解密。
//!
//! ## 保密边界
//!
//! 控制面搬运的 payload 一律是**不透明字节**：服务器既不解析也不解密。
//! 端到端加密由客户端负责 —— 服务器只是替不在线的收件人保管一个信封。

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::config::{DirectoryConfig, GroupConfig, OfflineConfig};
use crate::frame::TokenHash;

// ───────────────────────── 离线消息 ─────────────────────────

struct Stored {
    payload: Vec<u8>,
    stored_at: Instant,
}

/// 离线消息仓库：按收件人令牌哈希排队，服务器看不见内容。
pub struct OfflineStore {
    cfg: OfflineConfig,
    inner: Mutex<HashMap<TokenHash, VecDeque<Stored>>>,
}

#[derive(Debug)]
pub enum L4Error {
    Disabled,
    PayloadTooLarge { max: usize },
    NotFound,
    GroupFull { max: usize },
}

impl std::fmt::Display for L4Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            L4Error::Disabled => write!(f, "该功能在当前档位被关闭"),
            L4Error::PayloadTooLarge { max } => write!(f, "载荷超过上限 {max} 字节"),
            L4Error::NotFound => write!(f, "未找到"),
            L4Error::GroupFull { max } => write!(f, "群成员已达上限 {max}"),
        }
    }
}

impl OfflineStore {
    pub fn new(cfg: OfflineConfig) -> Self {
        Self { cfg, inner: Mutex::new(HashMap::new()) }
    }

    #[allow(dead_code)]
    pub fn enabled(&self) -> bool {
        self.cfg.enabled
    }

    /// 投递一条离线消息。返回该收件人当前积压条数。
    pub fn push(&self, dst: TokenHash, payload: Vec<u8>) -> Result<usize, L4Error> {
        if !self.cfg.enabled {
            return Err(L4Error::Disabled);
        }
        if payload.len() > self.cfg.max_payload {
            return Err(L4Error::PayloadTooLarge { max: self.cfg.max_payload });
        }
        let mut guard = self.inner.lock().expect("offline mutex poisoned");
        let q = guard.entry(dst).or_default();
        if q.len() >= self.cfg.max_per_recipient {
            // 先进先出：挤掉最旧的一条，保住新消息
            q.pop_front();
        }
        q.push_back(Stored { payload, stored_at: Instant::now() });
        Ok(q.len())
    }

    /// 取走某收件人的至多 max 条消息。取走即删除。
    pub fn pull(&self, dst: TokenHash, max: usize) -> Result<Vec<Vec<u8>>, L4Error> {
        if !self.cfg.enabled {
            return Err(L4Error::Disabled);
        }
        let mut guard = self.inner.lock().expect("offline mutex poisoned");
        let Some(q) = guard.get_mut(&dst) else {
            return Ok(Vec::new());
        };
        let take = q.len().min(max.max(1));
        let mut out = Vec::with_capacity(take);
        for _ in 0..take {
            if let Some(item) = q.pop_front() {
                out.push(item.payload);
            }
        }
        if q.is_empty() {
            guard.remove(&dst);
        }
        Ok(out)
    }

    /// 清理过期消息。返回清理条数。
    pub fn sweep(&self) -> usize {
        let ttl = Duration::from_secs(self.cfg.ttl_secs.max(1));
        let mut guard = self.inner.lock().expect("offline mutex poisoned");
        let mut removed = 0usize;
        guard.retain(|_, q| {
            let before = q.len();
            q.retain(|s| s.stored_at.elapsed() < ttl);
            removed += before - q.len();
            !q.is_empty()
        });
        removed
    }

    /// 积压总条数（诊断用，不含身份）。
    pub fn depth(&self) -> usize {
        self.inner.lock().expect("offline mutex poisoned").values().map(|q| q.len()).sum()
    }
}

// ───────────────────────── 群组 ─────────────────────────

struct Broadcast {
    payload: Vec<u8>,
    cursor: u64,
}

/// 群组：每个群维护「成员名单 + 追加日志」，成员各自按游标拉取。
/// 服务器不知道群名对应的真实身份 —— 成员一律用令牌哈希表示。
pub struct Groups {
    cfg: GroupConfig,
    inner: Mutex<HashMap<String, GroupState>>,
}

struct GroupState {
    members: HashSet<TokenHash>,
    log: VecDeque<Broadcast>,
    next_cursor: u64,
    cursors: HashMap<TokenHash, u64>,
}

impl GroupState {
    fn new() -> Self {
        Self {
            members: HashSet::new(),
            log: VecDeque::new(),
            next_cursor: 1,
            cursors: HashMap::new(),
        }
    }
}

impl Groups {
    pub fn new(cfg: GroupConfig) -> Self {
        Self { cfg, inner: Mutex::new(HashMap::new()) }
    }

    #[allow(dead_code)]
    pub fn enabled(&self) -> bool {
        self.cfg.enabled
    }

    pub fn create(&self, name: &str, owner: TokenHash) -> Result<(), L4Error> {
        if !self.cfg.enabled {
            return Err(L4Error::Disabled);
        }
        let mut guard = self.inner.lock().expect("groups mutex poisoned");
        let g = guard.entry(name.to_string()).or_insert_with(GroupState::new);
        if g.members.len() >= self.cfg.max_members && !g.members.contains(&owner) {
            return Err(L4Error::GroupFull { max: self.cfg.max_members });
        }
        g.members.insert(owner);
        g.cursors.entry(owner).or_insert(0);
        Ok(())
    }

    pub fn join(&self, name: &str, member: TokenHash) -> Result<usize, L4Error> {
        if !self.cfg.enabled {
            return Err(L4Error::Disabled);
        }
        let mut guard = self.inner.lock().expect("groups mutex poisoned");
        let Some(g) = guard.get_mut(name) else {
            return Err(L4Error::NotFound);
        };
        if !g.members.contains(&member) && g.members.len() >= self.cfg.max_members {
            return Err(L4Error::GroupFull { max: self.cfg.max_members });
        }
        g.members.insert(member);
        g.cursors.entry(member).or_insert(0);
        Ok(g.members.len())
    }

    /// 广播一条消息，返回游标号。
    pub fn send(&self, name: &str, sender: TokenHash, payload: Vec<u8>) -> Result<u64, L4Error> {
        if !self.cfg.enabled {
            return Err(L4Error::Disabled);
        }
        if payload.len() > self.cfg.max_payload {
            return Err(L4Error::PayloadTooLarge { max: self.cfg.max_payload });
        }
        let mut guard = self.inner.lock().expect("groups mutex poisoned");
        let Some(g) = guard.get_mut(name) else {
            return Err(L4Error::NotFound);
        };
        if !g.members.contains(&sender) {
            return Err(L4Error::NotFound);
        }
        let cursor = g.next_cursor;
        g.next_cursor += 1;
        g.log.push_back(Broadcast { payload, cursor });
        let cap = self.cfg.max_members.saturating_mul(64).max(64);
        while g.log.len() > cap {
            g.log.pop_front();
        }
        Ok(cursor)
    }

    /// 拉取自己游标之后的消息。
    pub fn pull(&self, name: &str, member: TokenHash) -> Result<Vec<(u64, Vec<u8>)>, L4Error> {
        if !self.cfg.enabled {
            return Err(L4Error::Disabled);
        }
        let mut guard = self.inner.lock().expect("groups mutex poisoned");
        let Some(g) = guard.get_mut(name) else {
            return Err(L4Error::NotFound);
        };
        if !g.members.contains(&member) {
            return Err(L4Error::NotFound);
        }
        let since = *g.cursors.get(&member).unwrap_or(&0);
        let mut out = Vec::new();
        for b in g.log.iter() {
            if b.cursor > since {
                out.push((b.cursor, b.payload.clone()));
            }
        }
        if let Some(last) = out.last() {
            g.cursors.insert(member, last.0);
        }
        Ok(out)
    }

    #[allow(dead_code)]
    pub fn member_count(&self, name: &str) -> Option<usize> {
        self.inner.lock().expect("groups mutex poisoned").get(name).map(|g| g.members.len())
    }
}

// ───────────────────────── 身份目录 ─────────────────────────

/// 身份目录：只回答「这个令牌哈希最近活跃过吗」，不回答「这是谁」。
/// 时间一律粗化成整分钟，默认禁止任意查询以抗流量分析。
pub struct Directory {
    cfg: DirectoryConfig,
    seen: Mutex<HashMap<TokenHash, Instant>>,
}

impl Directory {
    pub fn new(cfg: DirectoryConfig) -> Self {
        Self { cfg, seen: Mutex::new(HashMap::new()) }
    }

    #[allow(dead_code)]
    pub fn enabled(&self) -> bool {
        self.cfg.enabled
    }

    /// 是否允许任意哈希查询（默认 false，抗流量分析）。
    pub fn allow_arbitrary(&self) -> bool {
        self.cfg.allow_arbitrary_lookup
    }

    /// 数据面配对成功时调用，记录活跃。
    pub fn touch(&self, hash: TokenHash) {
        if !self.cfg.enabled {
            return;
        }
        let mut guard = self.seen.lock().expect("directory mutex poisoned");
        guard.insert(hash, Instant::now());
    }

    /// 查询。返回 (是否活跃, 最近活跃距今的粗化秒数)。
    pub fn lookup(&self, hash: TokenHash, allow_arbitrary: bool) -> Result<(bool, Option<u64>), L4Error> {
        if !self.cfg.enabled {
            return Err(L4Error::Disabled);
        }
        if !self.cfg.allow_arbitrary_lookup && !allow_arbitrary {
            return Err(L4Error::Disabled);
        }
        let guard = self.seen.lock().expect("directory mutex poisoned");
        match guard.get(&hash) {
            None => Ok((false, None)),
            Some(t) => Ok((true, Some(t.elapsed().as_secs() / 60 * 60))),
        }
    }

    /// 清理长期不活跃记录。
    pub fn sweep(&self, keep: Duration) -> usize {
        let mut guard = self.seen.lock().expect("directory mutex poisoned");
        let before = guard.len();
        guard.retain(|_, t| t.elapsed() < keep);
        before - guard.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(n: u8) -> TokenHash {
        TokenHash([n; 32])
    }

    fn offline_cfg() -> OfflineConfig {
        OfflineConfig { enabled: true, max_payload: 1024, max_per_recipient: 3, ttl_secs: 3600 }
    }

    #[test]
    fn offline_fifo_eviction() {
        let s = OfflineStore::new(offline_cfg());
        for i in 0..3u8 {
            s.push(h(1), vec![i]).unwrap();
        }
        let d = s.push(h(1), vec![9]).unwrap();
        assert_eq!(d, 3, "超过上限应挤掉最旧的，长度保持 3");
        let items = s.pull(h(1), 10).unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0], vec![1], "最旧的 0 应被挤掉");
        assert_eq!(items[2], vec![9]);
    }

    #[test]
    fn offline_rejects_oversize() {
        let s = OfflineStore::new(offline_cfg());
        let e = s.push(h(1), vec![0u8; 2048]);
        assert!(matches!(e, Err(L4Error::PayloadTooLarge { .. })));
    }

    #[test]
    fn group_cursor_advances() {
        let g = Groups::new(GroupConfig { enabled: true, max_members: 8, max_payload: 1024 });
        g.create("room", h(1)).unwrap();
        g.join("room", h(2)).unwrap();
        g.send("room", h(1), b"hello".to_vec()).unwrap();
        let got = g.pull("room", h(2)).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].1, b"hello");
        assert!(g.pull("room", h(2)).unwrap().is_empty(), "游标应已推进");
    }

    #[test]
    fn group_rejects_non_member() {
        let g = Groups::new(GroupConfig { enabled: true, max_members: 8, max_payload: 1024 });
        g.create("room", h(1)).unwrap();
        assert!(matches!(g.send("room", h(3), b"x".to_vec()), Err(L4Error::NotFound)));
    }

    #[test]
    fn directory_coarsens_time() {
        let d = Directory::new(DirectoryConfig { enabled: true, allow_arbitrary_lookup: true });
        d.touch(h(7));
        let (alive, ago) = d.lookup(h(7), true).unwrap();
        assert!(alive);
        assert_eq!(ago.map(|s| s % 60), Some(0), "时间必须粗化到整分钟");
    }

    #[test]
    fn directory_blocks_arbitrary_lookup_by_default() {
        let d = Directory::new(DirectoryConfig { enabled: true, allow_arbitrary_lookup: false });
        assert!(matches!(d.lookup(h(7), false), Err(L4Error::Disabled)));
    }

    #[test]
    fn disabled_features_refuse() {
        let s = OfflineStore::new(OfflineConfig { enabled: false, max_payload: 1, max_per_recipient: 1, ttl_secs: 1 });
        assert!(matches!(s.push(h(1), vec![1]), Err(L4Error::Disabled)));
    }
}
