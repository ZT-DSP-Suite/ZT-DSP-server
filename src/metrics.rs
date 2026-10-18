// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 ZT-DSP Suite contributors

//! 本地指标。**只记计数，绝不记身份。**
//!
//! 规范第 978 行【强制】：服务器运营者不得记录阶段一的配对信息。
//! 因此这里只有聚合计数，没有任何令牌哈希、IP 或会话映射。

use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Default)]
pub struct Metrics {
    pub accepted: AtomicU64,
    pub rejected_silent: AtomicU64,
    pub paired: AtomicU64,
    pub lobby_timeout: AtomicU64,
    pub bytes_in: AtomicU64,
    pub bytes_out: AtomicU64,
    pub offline_stored: AtomicU64,
    pub offline_delivered: AtomicU64,
    pub group_broadcasts: AtomicU64,
    pub active_pairs: AtomicU64,
    /// 当前存活连接总数（含未配对），用于全局连接上限
    pub active_conns: AtomicU64,
}

impl Metrics {
    pub fn inc(c: &AtomicU64) {
        c.fetch_add(1, Ordering::Relaxed);
    }

    pub fn add(c: &AtomicU64, n: u64) {
        c.fetch_add(n, Ordering::Relaxed);
    }

    pub fn get(c: &AtomicU64) -> u64 {
        c.load(Ordering::Relaxed)
    }
}
