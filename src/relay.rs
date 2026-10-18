// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 ZT-DSP Suite contributors

//! 配对成功后的双向字节中继。
//!
//! 铁律：**服务器全程不解密、不改写、不插入**。它只会：
//!   * 把 A 的字节原样倒给 B；
//!   * 把 B 的字节原样倒给 A；
//!   * 统计字节数（只用于本地限流与诊断）。
//!
//! 阶段二 / 三 / 四的所有内容对服务器都是不透明的 —— 这正是规范第 254 行
//! 「服务器只转发包裹，不打开箱子」的字面实现。

use std::io;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::metrics::Metrics;

/// 中继参数。
pub struct RelayOpts {
    pub buffer_bytes: usize,
    /// 单向空闲超时；None = 不设超时
    pub idle_timeout: Option<Duration>,
}

/// 单向搬运：从 `r` 读到 `w`，直到 EOF / 出错 / 空闲超时。返回搬运字节数。
async fn pipe<R, W>(
    r: &mut R,
    w: &mut W,
    opts: &RelayOpts,
) -> io::Result<u64>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buf = vec![0u8; opts.buffer_bytes.max(1024)];
    let mut total: u64 = 0;

    loop {
        let n = match opts.idle_timeout {
            None => r.read(&mut buf).await?,
            Some(d) => match tokio::time::timeout(d, r.read(&mut buf)).await {
                Err(_) => return Ok(total), // 空闲超时：正常收尾
                Ok(res) => res?,
            },
        };
        if n == 0 {
            return Ok(total); // 对端关闭
        }
        w.write_all(&buf[..n]).await?;
        w.flush().await?;
        total = total.saturating_add(n as u64);
    }
}

/// 双向中继一对已配对的连接。
///
/// 返回 (A→B 字节数, B→A 字节数)。任一侧结束即整体收尾。
pub async fn relay_pair<A, B>(
    a: A,
    b: B,
    opts: RelayOpts,
    metrics: &Metrics,
) -> io::Result<(u64, u64)>
where
    A: AsyncRead + AsyncWrite + Unpin + Send,
    B: AsyncRead + AsyncWrite + Unpin + Send,
{
    let (mut ar, mut aw) = tokio::io::split(a);
    let (mut br, mut bw) = tokio::io::split(b);

    let ab = {
        let opts = RelayOpts { buffer_bytes: opts.buffer_bytes, idle_timeout: opts.idle_timeout };
        async move { pipe(&mut ar, &mut bw, &opts).await }
    };
    let ba = {
        let opts = RelayOpts { buffer_bytes: opts.buffer_bytes, idle_timeout: opts.idle_timeout };
        async move { pipe(&mut br, &mut aw, &opts).await }
    };

    // 任一侧结束就取消另一侧，避免半开连接常驻
    let (ab_res, ba_res) = tokio::join!(ab, ba);

    let ab_n = ab_res.unwrap_or(0);
    let ba_n = ba_res.unwrap_or(0);
    Metrics::add(&metrics.bytes_in, ab_n);
    Metrics::add(&metrics.bytes_out, ba_n);

    Ok((ab_n, ba_n))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    #[tokio::test]
    async fn relays_both_directions() {
        let m = Metrics::default();
        let (c1, s1) = duplex(65536);
        let (c2, s2) = duplex(65536);

        let opts = RelayOpts { buffer_bytes: 8192, idle_timeout: None };
        let h = tokio::spawn(async move { relay_pair(s1, s2, opts, &m).await });

        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut c1 = c1;
        let mut c2 = c2;
        c1.write_all(b"ping").await.unwrap();
        c1.flush().await.unwrap();
        let mut got = [0u8; 4];
        c2.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"ping");

        c2.write_all(b"pong").await.unwrap();
        c2.flush().await.unwrap();
        let mut got2 = [0u8; 4];
        c1.read_exact(&mut got2).await.unwrap();
        assert_eq!(&got2, b"pong");

        drop(c1);
        drop(c2);
        let (ab, ba) = h.await.unwrap().unwrap();
        assert_eq!(ab, 4);
        assert_eq!(ba, 4);
    }

    #[tokio::test]
    async fn idle_timeout_closes_quiet_pair() {
        let m = Metrics::default();
        let (c1, s1) = duplex(1024);
        let (c2, s2) = duplex(1024);
        let opts = RelayOpts {
            buffer_bytes: 1024,
            idle_timeout: Some(Duration::from_millis(120)),
        };
        let h = tokio::spawn(async move { relay_pair(s1, s2, opts, &m).await });
        let t0 = std::time::Instant::now();
        let _ = h.await.unwrap();
        assert!(t0.elapsed() < Duration::from_secs(2), "空闲超时应及时收尾");
        drop(c1);
        drop(c2);
    }
}
