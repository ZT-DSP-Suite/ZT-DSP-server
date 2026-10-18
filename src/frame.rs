// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 ZT-DSP Suite contributors

//! 附录 F 逐字节帧规范 —— 服务端只需要认得两样东西：
//!   1. 阶段一帧里的**身份宣告**（源/目标令牌哈希），用于等候池配对；
//!   2. 阶段四包的**会话头**（session_id），用于把包投递到正确房间。
//! 其余字节一律原样转发，服务器全程不解密。
//!
//! 字节序全为大端。变长字段统一 `[长度:4B][载荷]`。

// 本模块导出的是**完整协议 API 面**：阶段四路由相关的解析器与常量目前
// 只被测试与未来的多房间投递路径使用，因此允许暂未在主线调用。
#![allow(dead_code)]

use std::fmt;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::time::{timeout_at, Instant};

// ── 附录 F.6 硬上限 ────────────────────────────────────────────────
/// 阶段一参数 JSON 上限
pub const PARAM_JSON_MAX: u32 = 4096;
/// 身份宣告上限
pub const IDENTITY_MAX: u32 = 8192;
/// nonce 上限
pub const NONCE_MAX: u32 = 512;
/// 阶段四密文上限
pub const CIPHERTEXT_MAX: u32 = 1_048_576;

/// 令牌哈希长度（恒 32B）
pub const TOKEN_HASH_LEN: usize = 32;
/// 身份宣告最小长度：src_hash ‖ dst_hash = 64B
pub const IDENTITY_MIN: u32 = 64;
/// 会话 ID 长度（恒 32B）
pub const SESSION_ID_LEN: usize = 32;
/// 阶段四明文头长度：session_id(32) + seq(8) + ct_len(4) = 44B
pub const PHASE4_HEADER_LEN: usize = 44;
/// 阶段四载荷最小长度（用户明文为空时）= ts(8)+fmt(1)+dst(32)+src(32)+next_seed(32)
pub const PAYLOAD_MIN: usize = 105;

/// 单个令牌哈希。
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TokenHash(pub [u8; TOKEN_HASH_LEN]);

impl fmt::Debug for TokenHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 只打印前 4 字节，避免日志里出现完整身份标识
        write!(f, "TokenHash({:02x}{:02x}{:02x}{:02x}…)", self.0[0], self.0[1], self.0[2], self.0[3])
    }
}

impl TokenHash {
    pub fn hex_prefix(&self, n: usize) -> String {
        self.0[..n.min(TOKEN_HASH_LEN)].iter().map(|b| format!("{b:02x}")).collect()
    }
}

/// 身份宣告解释出的源 / 目标令牌哈希。
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Identity {
    pub src: TokenHash,
    pub dst: TokenHash,
}

impl fmt::Debug for Identity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Identity{{src:{:?}, dst:{:?}}}", self.src, self.dst)
    }
}

/// 帧解析错误。服务器侧一律走「静默丢弃」，对外不产生任何可观测差异。
#[derive(Debug)]
pub enum FrameError {
    /// 对端干净关闭
    Eof,
    /// 共享截止时间耗尽（Slowloris 防护）
    Timeout,
    /// 长度字段超限
    LengthExceeded(&'static str),
    /// 身份宣告太短，无法解释出 src/dst
    IdentityTooShort,
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FrameError::Eof => write!(f, "eof"),
            FrameError::Timeout => write!(f, "timeout"),
            FrameError::LengthExceeded(what) => write!(f, "length exceeded: {what}"),
            FrameError::IdentityTooShort => write!(f, "identity too short"),
        }
    }
}

impl std::error::Error for FrameError {}

/// 从流里读满 `buf`，全部字节共享同一个截止时间。
///
/// 这是 F.2 / H.3 的 **Slowloris 防护**要求：整帧共享一个截止时间，
/// 而不是「每次 recv 重新计时」——否则攻击者每 14 秒发 1 字节就能无限拖延。
async fn read_exact_deadline<R: AsyncRead + Unpin>(
    r: &mut R,
    buf: &mut [u8],
    deadline: Instant,
) -> Result<(), FrameError> {
    match timeout_at(deadline, r.read_exact(buf)).await {
        Err(_) => Err(FrameError::Timeout),
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => Err(FrameError::Eof),
        Ok(Err(_)) => Err(FrameError::Eof),
        Ok(Ok(_)) => Ok(()),
    }
}

async fn read_u32_be<R: AsyncRead + Unpin>(
    r: &mut R,
    deadline: Instant,
) -> Result<u32, FrameError> {
    let mut b = [0u8; 4];
    read_exact_deadline(r, &mut b, deadline).await?;
    Ok(u32::from_be_bytes(b))
}

/// 阶段一帧（完整原始字节 + 解释结果）。
#[derive(Clone)]
pub struct Phase1Frame {
    /// 原始帧字节，**含全部长度前缀**。配对后必须原样转给对端 ——
    /// 这些字节会进入阶段三的 transcript，任何重编码都会让标签校验失败。
    pub raw: Vec<u8>,
    /// 参数 JSON 原文（服务端不解析内容，只做长度与合法性占位）
    pub param_json: Vec<u8>,
    /// 解释出的身份
    pub identity: Identity,
    /// 接入随机数（服务端不校验，只透传）
    pub nonce: Vec<u8>,
}

/// 读一整帧阶段一。
///
/// `deadline` 是**整帧共享**的截止时间，由调用方按握手超时（默认 15s）计算。
pub async fn read_phase1<R: AsyncRead + Unpin>(
    r: &mut R,
    deadline: Instant,
) -> Result<Phase1Frame, FrameError> {
    let mut raw: Vec<u8> = Vec::with_capacity(256);

    // ── param_json ──
    let param_len = read_u32_be(r, deadline).await?;
    if param_len > PARAM_JSON_MAX {
        return Err(FrameError::LengthExceeded("param_json"));
    }
    raw.extend_from_slice(&param_len.to_be_bytes());
    let mut param_json = vec![0u8; param_len as usize];
    if param_len > 0 {
        read_exact_deadline(r, &mut param_json, deadline).await?;
    }
    raw.extend_from_slice(&param_json);

    // ── identity_blob ──
    let identity_len = read_u32_be(r, deadline).await?;
    if identity_len > IDENTITY_MAX {
        return Err(FrameError::LengthExceeded("identity_blob"));
    }
    if identity_len < IDENTITY_MIN {
        return Err(FrameError::IdentityTooShort);
    }
    raw.extend_from_slice(&identity_len.to_be_bytes());
    let mut identity_blob = vec![0u8; identity_len as usize];
    read_exact_deadline(r, &mut identity_blob, deadline).await?;
    raw.extend_from_slice(&identity_blob);

    // ── nonce ──
    let nonce_len = read_u32_be(r, deadline).await?;
    if nonce_len > NONCE_MAX {
        return Err(FrameError::LengthExceeded("nonce"));
    }
    raw.extend_from_slice(&nonce_len.to_be_bytes());
    let mut nonce = vec![0u8; nonce_len as usize];
    if nonce_len > 0 {
        read_exact_deadline(r, &mut nonce, deadline).await?;
    }
    raw.extend_from_slice(&nonce);

    // 身份宣告前 64B = src_hash ‖ dst_hash
    let mut src = [0u8; TOKEN_HASH_LEN];
    let mut dst = [0u8; TOKEN_HASH_LEN];
    src.copy_from_slice(&identity_blob[..TOKEN_HASH_LEN]);
    dst.copy_from_slice(&identity_blob[TOKEN_HASH_LEN..2 * TOKEN_HASH_LEN]);

    Ok(Phase1Frame {
        raw,
        param_json,
        identity: Identity { src: TokenHash(src), dst: TokenHash(dst) },
        nonce,
    })
}

/// 阶段四会话头（仅当需要按 session_id 做多房间投递时才解析）。
#[derive(Debug, Clone, Copy)]
pub struct Phase4Header {
    pub session_id: [u8; SESSION_ID_LEN],
    pub seq: u64,
    pub ct_len: u32,
}

/// 解析 44B 会话头。
pub fn parse_phase4_header(buf: &[u8]) -> Option<Phase4Header> {
    if buf.len() < PHASE4_HEADER_LEN {
        return None;
    }
    let mut sid = [0u8; SESSION_ID_LEN];
    sid.copy_from_slice(&buf[..SESSION_ID_LEN]);
    let seq = u64::from_be_bytes(buf[32..40].try_into().ok()?);
    let ct_len = u32::from_be_bytes(buf[40..44].try_into().ok()?);
    if ct_len > CIPHERTEXT_MAX {
        return None;
    }
    Some(Phase4Header { session_id: sid, seq, ct_len })
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::time::Duration;

    /// 构造一帧合法的阶段一帧（默认参数 120B + 身份 64B + nonce 32B = 228B）
    fn sample_phase1() -> Vec<u8> {
        let param = br#"{"aead":"AES-256-GCM","hash":"SHA3-256","key_len":256,"module_flags":{},"random_len":256,"random_rate":1024,"window":60}"#;
        assert_eq!(param.len(), 120, "默认参数 JSON 必须恰好 120 字节");
        let mut buf = Vec::new();
        buf.extend_from_slice(&(param.len() as u32).to_be_bytes());
        buf.extend_from_slice(param);
        buf.extend_from_slice(&64u32.to_be_bytes());
        buf.extend_from_slice(&[0xAAu8; 32]); // src
        buf.extend_from_slice(&[0xBBu8; 32]); // dst
        buf.extend_from_slice(&32u32.to_be_bytes());
        buf.extend_from_slice(&[0xCCu8; 32]); // nonce
        buf
    }

    #[tokio::test]
    async fn parses_default_phase1() {
        let raw = sample_phase1();
        assert_eq!(raw.len(), 228, "默认阶段一帧必须 228 字节");
        let mut c = Cursor::new(raw);
        let f = read_phase1(&mut c, Instant::now() + Duration::from_secs(5)).await.unwrap();
        assert_eq!(f.raw.len(), 228);
        assert_eq!(f.identity.src.0, [0xAA; 32]);
        assert_eq!(f.identity.dst.0, [0xBB; 32]);
        assert_eq!(f.nonce.len(), 32);
    }

    #[tokio::test]
    async fn rejects_oversized_param() {
        let mut raw = (PARAM_JSON_MAX + 1).to_be_bytes().to_vec();
        raw.extend_from_slice(&[0u8; 8]);
        let mut c = Cursor::new(raw);
        let e = read_phase1(&mut c, Instant::now() + Duration::from_secs(5)).await;
        assert!(matches!(e, Err(FrameError::LengthExceeded("param_json"))));
    }

    #[tokio::test]
    async fn rejects_short_identity() {
        let mut raw = 2u32.to_be_bytes().to_vec();
        raw.extend_from_slice(b"{}");
        raw.extend_from_slice(&32u32.to_be_bytes()); // identity_len = 32 < 64
        raw.extend_from_slice(&[0u8; 32]);
        let mut c = Cursor::new(raw);
        let e = read_phase1(&mut c, Instant::now() + Duration::from_secs(5)).await;
        assert!(matches!(e, Err(FrameError::IdentityTooShort)));
    }

    #[tokio::test]
    async fn phase4_header_roundtrip() {
        let mut h = Vec::new();
        h.extend_from_slice(&[0x11u8; 32]);
        h.extend_from_slice(&7u64.to_be_bytes());
        h.extend_from_slice(&165u32.to_be_bytes());
        let p = parse_phase4_header(&h).unwrap();
        assert_eq!(p.session_id, [0x11u8; 32]);
        assert_eq!(p.seq, 7);
        assert_eq!(p.ct_len, 165);
    }

    #[test]
    fn debug_never_prints_full_hash() {
        let t = TokenHash([0xAB; 32]);
        let s = format!("{t:?}");
        assert!(s.len() < 40, "调试输出不应包含完整 32 字节: {s}");
        assert!(!s.contains("abababababababab"));
    }
}
