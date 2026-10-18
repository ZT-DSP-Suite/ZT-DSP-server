// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 ZT-DSP Suite contributors

//! ZT-DSP 1.0.1 服务端：登记 → 配对 → 中继，全程不解密。
//!
//! 本文件是**单文件实现**（收录口径：一实现一文件，主线目录不新增目录树）。
//! 各子模块以内联 `mod` 保留原有边界与可见性。

/// 版本契约（单一事实来源）。
///
/// 与 `zt_dsp_py/version.py`、`zt_dsp_rs/version.rs` 保持同一套声明形状，
/// 差别只在角色前缀：客户端是语言前缀（PY/RS），服务端是角色前缀（SV）。
///
/// 握手帧**不携带任何版本串**：阶段一参数 JSON 与阶段四 AAD 都不含版本或协议标识字段。
mod version {
    /// 代码侧双向声明：本实现所遵循的文档。
    pub const FOLLOWS_DOC: &str = "ZT-DSP Protocol Master Guide";

    /// 代码侧双向声明：所遵循文档的路径。
    // 注意：该路径指向**同级仓库** ZT-DSP-docs，在本仓库内不可解析。
    pub const FOLLOWS_DOC_PATH: &str = "ZT-DSP-docs/en/spec/ZT-DSP-1.0.1-Master_Guide.md";

    /// 本实现适用的总指导版本（多对多，可多项）。
    pub const APPLIES_TO_DOC_VERSIONS: &[&str] = &["1.0.0", "1.0.1"];

    /// 总指导版本号（仅用于文档修订计数，不参与网络通信）。
    pub const DOCS_VERSION: &str = "1.0.1";

    /// 本实现的角色。
    pub const ROLE: &str = "server";

    /// 角色前缀（服务端用角色而非语言）。
    pub const ROLE_PREFIX: &str = "SV";

    /// 实现版本号。
    pub const CODE_VERSION: &str = "1.0.0";

    /// 实现标识 `ZT-DSP_SV-1.0.0`。
    pub const IMPLEMENTATION_ID: &str = concat!("ZT-DSP_", "SV", "-", "1.0.0");

    /// 适用同一总指导版本的客户端版本。
    pub const CLIENT_VERSIONS: &[&str] = &["ZT-DSP_PY-1.0.0", "ZT-DSP_RS-1.0.0"];

    /// 适用同一总指导版本的服务端版本。
    pub const SERVER_VERSIONS: &[&str] = &["ZT-DSP_SV-1.0.0"];

    /// CLI `--version` 的单行输出。
    pub fn format_version() -> String {
        format!("{} (docs {})", IMPLEMENTATION_ID, DOCS_VERSION)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn version_contract() {
            assert_eq!(IMPLEMENTATION_ID, "ZT-DSP_SV-1.0.0");
            assert_eq!(DOCS_VERSION, "1.0.1");
            assert_eq!(CODE_VERSION, "1.0.0");
            assert_eq!(ROLE, "server");
            assert_eq!(ROLE_PREFIX, "SV");
        }

        #[test]
        fn bidirectional_declaration_is_present() {
            assert!(!FOLLOWS_DOC.is_empty());
            assert!(FOLLOWS_DOC_PATH.contains("ZT-DSP-docs"));
            assert!(!APPLIES_TO_DOC_VERSIONS.is_empty());
            assert!(APPLIES_TO_DOC_VERSIONS.contains(&DOCS_VERSION));
        }

        /// 复用 client 仓的共享一致性向量。
        ///
        /// 服务端不解密、不派生密钥，9 个向量里绝大多数对它没有直接约束。
        /// 但派生标签的命名规则是服务端必须与规范保持一致的间接契约：
        /// 只要客户端与服务端对 -v3 后缀的理解分叉，双方就永远通不了。
        #[test]
        fn shared_vectors_use_the_v3_suffix() {
            let raw = include_str!("../test-vectors/vectors.json");
            let v: serde_json::Value = serde_json::from_str(raw).expect("vectors.json 必须可解析");
            assert_eq!(v["schema"], "zt-dsp-shared-vectors/1");

            // INFO_EK_ROTATE 在上游 vectors.json 里就没有 -v3 后缀。
            // 这是本仓观察到的上游数据不一致，不是本实现的选择；
            // 已单列并在 PR 里向文档仓 owner 提问，对方确认前不擅自改上游数据。
            const KNOWN_UPSTREAM_EXCEPTIONS: &[&str] = &["INFO_EK_ROTATE"];

            let labels = v["labels"].as_object().expect("labels 必须是对象");
            assert!(!labels.is_empty(), "labels 不得为空");
            for (name, value) in labels {
                let label = value.as_str().expect("label 必须是字符串");
                if KNOWN_UPSTREAM_EXCEPTIONS.contains(&name.as_str()) {
                    continue;
                }
                assert!(label.ends_with("-v3"), "{name} = {label} 缺少 -v3 后缀");
            }
        }

        #[test]
        fn format_version_is_single_line() {
            let s = format_version();
            assert!(s.starts_with("ZT-DSP_SV-1.0.0"));
            assert!(!s.contains('\n'));
        }
    }
}

/// 取互斥量守卫，中毒时取回内值。
///
/// 持锁区间内没有 unwrap、没有用户输入处理、没有分配失败传播，中毒只可能来自
/// 「持锁任务被取消」。这种情形下计数器与映射仍自洽，恢复优于让连接崩掉 ——
/// 静默丢弃契约本来就要求异常对端无感。
fn lock<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

mod util {
    //! 小工具：十六进制编解码。不引入额外依赖，保持二进制干净可移植。

    /// 字节 → 小写十六进制
    pub fn to_hex(bytes: &[u8]) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut s = String::with_capacity(bytes.len() * 2);
        for &b in bytes {
            s.push(HEX[(b >> 4) as usize] as char);
            s.push(HEX[(b & 0x0f) as usize] as char);
        }
        s
    }

    /// 十六进制 → 字节。非法字符或奇数长度返回 None。
    pub fn from_hex(s: &str) -> Option<Vec<u8>> {
        if s.len() % 2 != 0 {
            return None;
        }
        fn nibble(c: u8) -> Option<u8> {
            match c {
                b'0'..=b'9' => Some(c - b'0'),
                b'a'..=b'f' => Some(c - b'a' + 10),
                b'A'..=b'F' => Some(c - b'A' + 10),
                _ => None,
            }
        }
        let bytes = s.as_bytes();
        let mut out = Vec::with_capacity(bytes.len() / 2);
        for pair in bytes.chunks_exact(2) {
            out.push((nibble(pair[0])? << 4) | nibble(pair[1])?);
        }
        Some(out)
    }

    /// 解析 32 字节令牌哈希（64 个十六进制字符）。
    pub fn parse_token_hash(s: &str) -> Option<crate::frame::TokenHash> {
        let bytes = from_hex(s)?;
        if bytes.len() != 32 {
            return None;
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Some(crate::frame::TokenHash(arr))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn hex_roundtrip() {
            let data = [0x00u8, 0x0f, 0xa5, 0xff, 0x10];
            let h = to_hex(&data);
            assert_eq!(h, "000fa5ff10");
            assert_eq!(from_hex(&h).unwrap(), data);
        }

        #[test]
        fn hex_rejects_bad_input() {
            assert!(from_hex("abc").is_none(), "奇数长度应拒绝");
            assert!(from_hex("zz").is_none(), "非法字符应拒绝");
        }

        #[test]
        fn token_hash_requires_32_bytes() {
            assert!(parse_token_hash(&"ab".repeat(32)).is_some());
            assert!(parse_token_hash(&"ab".repeat(16)).is_none());
        }
    }
}

mod metrics {
    //! 本地指标。只记聚合计数：不含令牌哈希、会话 ID、载荷。
    //!
    //! 规范第 978 行【强制】禁止记录阶段一的配对信息。本模块因此只有聚合计数。
    //! 注意：对端套接字地址会出现在 `debug` 级日志里（运维定位用），`info` 级不含。

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
}

mod silent {
    //! 严格静默丢弃（附录 F.2）—— 服务器侧的门禁行为。
    //!
    //! 规范要求五要素缺一不可：
    //!
    //! 1. 不发送任何字节
    //! 2. 不主动关闭连接（FIN / RST 本身就是信号）
    //! 3. 以 8192B 为块持续读并丢弃，挂起到截止时间
    //! 4. 诊断只进本地 stderr，绝不进网络侧
    //! 5. 网络观感与「关闭的端口」不可区分
    //!
    //! 服务端的职责边界（总指导第 972 行）：服务器最坏只能拒转 / 延迟 / 丢包，
    //! 这些都是可用性攻击，偷不到内容。所以门禁判定失败时，静默丢弃就是正确响应。

    use std::time::Duration;

    use tokio::io::{AsyncRead, AsyncReadExt};
    use tokio::time::{timeout_at, Instant};

    /// 丢弃时的读取块大小（附录 F.2 指定 8192）
    pub const DRAIN_CHUNK: usize = 8192;

    /// 静默丢弃：读走并扔掉对端的字节，直到截止时间或对端关闭。
    ///
    /// 返回被丢弃的字节数（只用于本地诊断与限流，绝不回给对端）。
    pub async fn drain_silently<R: AsyncRead + Unpin>(r: &mut R, duration: Duration) -> u64 {
        let deadline = Instant::now() + duration;
        let mut buf = [0u8; DRAIN_CHUNK];
        let mut drained: u64 = 0;

        loop {
            match timeout_at(deadline, r.read(&mut buf)).await {
                // 对端干净关闭
                Ok(Ok(0)) => break,
                // 继续吞
                Ok(Ok(n)) => drained = drained.saturating_add(n as u64),
                // 连接已坏（RST 等）
                Ok(Err(_)) => break,
                // 截止时间到
                Err(_) => break,
            }
        }

        drained
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use tokio::io::duplex;

        #[tokio::test]
        async fn drains_without_writing() {
            let (mut a, mut b) = duplex(65536);

            // 对端猛灌 20000 字节
            let writer = tokio::spawn(async move {
                use tokio::io::AsyncWriteExt;
                let payload = vec![0x5Au8; 20_000];
                let _ = b.write_all(&payload).await;
                payload.len()
            });

            let drained = drain_silently(&mut a, Duration::from_millis(300)).await;
            let sent = writer.await.unwrap();
            assert!(drained > 0, "应该真的读走了字节");
            assert!(drained <= sent as u64 + DRAIN_CHUNK as u64);
        }

        #[tokio::test]
        async fn returns_promptly_on_peer_close() {
            let (mut a, b) = duplex(1024);
            drop(b);
            let t0 = std::time::Instant::now();
            let n = drain_silently(&mut a, Duration::from_secs(10)).await;
            assert_eq!(n, 0);
            assert!(
                t0.elapsed() < Duration::from_secs(2),
                "对端关闭应立即返回，不该干等 10 秒"
            );
        }
    }
}

mod frame {
    //! 附录 F 逐字节帧规范。
    //!
    //! 服务器只解释**阶段一帧**：参数集与身份宣告。
    //! 阶段二/三/四对服务器是不透明字节，原样转发，不解析、不解密。
    //!
    //! 字节序全为大端。变长字段统一 `[长度:4B][载荷]`。

    use std::fmt;

    use serde::Deserialize;
    use tokio::io::{AsyncRead, AsyncReadExt};
    use tokio::time::{timeout_at, Instant};

    /// 阶段一参数 JSON 上限（附录 F.6）
    pub const PARAM_JSON_MAX: u32 = 4096;
    /// 身份宣告上限（附录 F.6）
    pub const IDENTITY_MAX: u32 = 8192;
    /// nonce 上限（附录 F.6）
    pub const NONCE_MAX: u32 = 512;

    /// 令牌哈希长度（恒 32B）
    pub const TOKEN_HASH_LEN: usize = 32;
    /// 身份宣告最小长度：src_hash ‖ dst_hash = 64B
    pub const IDENTITY_MIN: u32 = 64;

    /// 单个令牌哈希。
    #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
    pub struct TokenHash(pub [u8; TOKEN_HASH_LEN]);

    impl fmt::Debug for TokenHash {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            // 只打印前 4 字节：完整哈希属于身份材料，不得进日志
            write!(
                f,
                "TokenHash({:02x}{:02x}{:02x}{:02x}…)",
                self.0[0], self.0[1], self.0[2], self.0[3]
            )
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

    /// 阶段一参数集（附录 F.1，默认 7 项）。
    #[derive(Clone, Debug, Deserialize)]
    pub struct ParamSet {
        pub aead: String,
        pub hash: String,
        pub key_len: u16,
        pub module_flags: serde_json::Map<String, serde_json::Value>,
        /// 接入随机数长度，**以比特计**
        pub random_len: u16,
        pub random_rate: u32,
        pub window: u32,
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
        /// 参数集非法：JSON 无效、缺项、取值越界
        InvalidParam(&'static str),
        /// nonce 实际长度与声明的 random_len 不一致
        NonceLengthMismatch {
            declared_bits: u16,
            actual_bits: u32,
        },
        /// 源与目标是同一个令牌哈希
        SelfAddressed,
    }

    impl fmt::Display for FrameError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                FrameError::Eof => write!(f, "eof"),
                FrameError::Timeout => write!(f, "timeout"),
                FrameError::LengthExceeded(w) => write!(f, "length exceeded: {w}"),
                FrameError::IdentityTooShort => write!(f, "identity too short"),
                FrameError::InvalidParam(w) => write!(f, "invalid param: {w}"),
                FrameError::NonceLengthMismatch {
                    declared_bits,
                    actual_bits,
                } => {
                    write!(f, "nonce length mismatch: declared {declared_bits} bits, actual {actual_bits} bits")
                }
                FrameError::SelfAddressed => write!(f, "source and destination are identical"),
            }
        }
    }

    impl std::error::Error for FrameError {}

    /// 从流里读满 `buf`，全部字节共享同一个截止时间。
    async fn read_exact_deadline<R: AsyncRead + Unpin>(
        r: &mut R,
        buf: &mut [u8],
        deadline: Instant,
    ) -> Result<(), FrameError> {
        match timeout_at(deadline, r.read_exact(buf)).await {
            Err(_) => Err(FrameError::Timeout),
            // 守卫臂与兜底臂结果相同（都是 Eof），留着只会让人以为两者有区别
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

    /// 附录 F.6：module_flags 深度上限与节点数上限。
    fn check_flags_depth(
        v: &serde_json::Value,
        depth: usize,
        nodes: &mut usize,
    ) -> Result<(), FrameError> {
        if depth > 32 {
            return Err(FrameError::InvalidParam("module_flags deeper than 32"));
        }
        *nodes += 1;
        if *nodes > 1024 {
            return Err(FrameError::InvalidParam(
                "module_flags has more than 1024 nodes",
            ));
        }
        match v {
            serde_json::Value::Object(m) => {
                for (_, x) in m {
                    check_flags_depth(x, depth + 1, nodes)?;
                }
            }
            serde_json::Value::Array(a) => {
                for x in a {
                    check_flags_depth(x, depth + 1, nodes)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// 校验参数集取值。
    fn validate_params(p: &ParamSet, nonce_len: u32) -> Result<(), FrameError> {
        let mut nodes = 0usize;
        for (_, v) in &p.module_flags {
            check_flags_depth(v, 1, &mut nodes)?;
        }
        if p.aead.is_empty() {
            return Err(FrameError::InvalidParam("aead empty"));
        }
        if p.hash.is_empty() {
            return Err(FrameError::InvalidParam("hash empty"));
        }
        match p.key_len {
            128 | 192 | 256 => {}
            _ => return Err(FrameError::InvalidParam("key_len not 128/192/256")),
        }
        if p.random_len == 0 || p.random_len % 8 != 0 {
            return Err(FrameError::InvalidParam(
                "random_len not a positive multiple of 8",
            ));
        }
        if p.random_len as u32 > NONCE_MAX * 8 {
            return Err(FrameError::InvalidParam("random_len exceeds nonce cap"));
        }
        if p.random_rate == 0 {
            return Err(FrameError::InvalidParam("random_rate zero"));
        }
        if p.window == 0 {
            return Err(FrameError::InvalidParam("window zero"));
        }
        // 附录 F.1：nonce 长度 = random_len / 8
        let expected = p.random_len as u32 / 8;
        if nonce_len != expected {
            return Err(FrameError::NonceLengthMismatch {
                declared_bits: p.random_len,
                actual_bits: nonce_len.saturating_mul(8),
            });
        }
        Ok(())
    }

    /// 阶段一帧（完整原始字节 + 解释结果）。
    #[derive(Clone)]
    pub struct Phase1Frame {
        /// 原始帧字节，含全部长度前缀；配对后原样转给对端
        pub raw: Vec<u8>,
        /// 已通过校验的参数集
        pub param: ParamSet,
        /// 解释出的身份
        pub identity: Identity,
    }

    impl Phase1Frame {
        /// 原始帧长度（诊断用）
        pub fn raw_len(&self) -> usize {
            self.raw.len()
        }
    }

    /// 读一整帧阶段一，并执行附录 F.2 要求的参数核对。
    pub async fn read_phase1<R: AsyncRead + Unpin>(
        r: &mut R,
        deadline: Instant,
    ) -> Result<Phase1Frame, FrameError> {
        let mut raw: Vec<u8> = Vec::with_capacity(256);

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

        // 参数集必须是合法 UTF-8 JSON
        let json = std::str::from_utf8(&param_json)
            .map_err(|_| FrameError::InvalidParam("param_json is not utf-8"))?;
        let param: ParamSet = serde_json::from_str(json)
            .map_err(|_| FrameError::InvalidParam("param_json is not a valid parameter set"))?;
        validate_params(&param, nonce_len)?;

        let mut src = [0u8; TOKEN_HASH_LEN];
        let mut dst = [0u8; TOKEN_HASH_LEN];
        src.copy_from_slice(&identity_blob[..TOKEN_HASH_LEN]);
        dst.copy_from_slice(&identity_blob[TOKEN_HASH_LEN..2 * TOKEN_HASH_LEN]);
        if src == dst {
            return Err(FrameError::SelfAddressed);
        }

        let _ = nonce; // 长度已校验；nonce 内容对服务器不透明，不保留
        Ok(Phase1Frame {
            raw,
            param,
            identity: Identity {
                src: TokenHash(src),
                dst: TokenHash(dst),
            },
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::io::Cursor;
        use std::time::Duration;

        fn dl() -> Instant {
            Instant::now() + Duration::from_secs(5)
        }

        const DEFAULT_PARAM: &str = r#"{"aead":"AES-256-GCM","hash":"SHA3-256","key_len":256,"module_flags":{},"random_len":256,"random_rate":1024,"window":60}"#;

        fn build(param: &str, src: u8, dst: u8, nonce_len: u32) -> Vec<u8> {
            let mut b = Vec::new();
            b.extend_from_slice(&(param.len() as u32).to_be_bytes());
            b.extend_from_slice(param.as_bytes());
            b.extend_from_slice(&64u32.to_be_bytes());
            b.extend_from_slice(&[src; 32]);
            b.extend_from_slice(&[dst; 32]);
            b.extend_from_slice(&nonce_len.to_be_bytes());
            b.extend_from_slice(&vec![0xCCu8; nonce_len as usize]);
            b
        }

        #[tokio::test]
        async fn parses_default_phase1() {
            assert_eq!(DEFAULT_PARAM.len(), 120);
            let raw = build(DEFAULT_PARAM, 0xAA, 0xBB, 32);
            assert_eq!(raw.len(), 228);
            let mut c = Cursor::new(raw);
            let f = read_phase1(&mut c, dl()).await.unwrap();
            assert_eq!(f.raw_len(), 228);
            assert_eq!(f.identity.src.0, [0xAA; 32]);
            assert_eq!(f.identity.dst.0, [0xBB; 32]);
            assert_eq!(f.param.key_len, 256);
            assert_eq!(f.param.random_len, 256);
        }

        #[tokio::test]
        async fn rejects_non_json_param() {
            let mut c = Cursor::new(build("not json", 0xAA, 0xBB, 0));
            assert!(matches!(
                read_phase1(&mut c, dl()).await,
                Err(FrameError::InvalidParam(_))
            ));
        }

        #[tokio::test]
        async fn rejects_missing_required_key() {
            let p = r#"{"aead":"AES-256-GCM","hash":"SHA3-256","key_len":256,"module_flags":{},"random_len":256,"random_rate":1024}"#;
            let mut c = Cursor::new(build(p, 0xAA, 0xBB, 32));
            assert!(matches!(
                read_phase1(&mut c, dl()).await,
                Err(FrameError::InvalidParam(_))
            ));
        }

        #[tokio::test]
        async fn rejects_nonce_not_matching_random_len() {
            // 声明 random_len=256（即 32B nonce），实际只给 16B
            let mut c = Cursor::new(build(DEFAULT_PARAM, 0xAA, 0xBB, 16));
            assert!(matches!(
                read_phase1(&mut c, dl()).await,
                Err(FrameError::NonceLengthMismatch { .. })
            ));
        }

        #[tokio::test]
        async fn rejects_bad_key_len() {
            let p = r#"{"aead":"AES-256-GCM","hash":"SHA3-256","key_len":999,"module_flags":{},"random_len":256,"random_rate":1024,"window":60}"#;
            let mut c = Cursor::new(build(p, 0xAA, 0xBB, 32));
            assert!(matches!(
                read_phase1(&mut c, dl()).await,
                Err(FrameError::InvalidParam(_))
            ));
        }

        #[tokio::test]
        async fn rejects_self_addressed() {
            let mut c = Cursor::new(build(DEFAULT_PARAM, 0xAA, 0xAA, 32));
            assert!(matches!(
                read_phase1(&mut c, dl()).await,
                Err(FrameError::SelfAddressed)
            ));
        }

        #[tokio::test]
        async fn rejects_oversized_param_length() {
            let mut raw = (PARAM_JSON_MAX + 1).to_be_bytes().to_vec();
            raw.extend_from_slice(&[0u8; 8]);
            let mut c = Cursor::new(raw);
            assert!(matches!(
                read_phase1(&mut c, dl()).await,
                Err(FrameError::LengthExceeded("param_json"))
            ));
        }

        #[tokio::test]
        async fn rejects_short_identity() {
            let mut raw = 2u32.to_be_bytes().to_vec();
            raw.extend_from_slice(b"{}");
            raw.extend_from_slice(&32u32.to_be_bytes());
            raw.extend_from_slice(&[0u8; 32]);
            let mut c = Cursor::new(raw);
            assert!(matches!(
                read_phase1(&mut c, dl()).await,
                Err(FrameError::IdentityTooShort)
            ));
        }

        #[test]
        fn debug_never_prints_full_hash() {
            let t = TokenHash([0xAB; 32]);
            let s = format!("{t:?}");
            assert!(s.len() < 40);
            assert!(!s.contains("abababababababab"));
        }
    }
}

mod lobby {
    //! 等候池（规范里叫「大厅」/「匹配池」）—— 酒店的等人逻辑。
    //!
    //! 依据总指导 4.2 服务器处理流程与第 613 行：
    //! > 查找等候池中是否有持有 H(Token_A) 的住客（即 B 已经先到，在等 A）。
    //! > 如果有 → 将 A 和 B 配对，送入同一房间；如果没有 → 将 A 放入等候池，等待 B 到来。
    //!
    //! 配对判据是**双向匹配**，杜绝「名字撞上就接通」的身份冒用：
    //!
    //!    新来者.src == 已在等者.dst  且  新来者.dst == 已在等者.src
    //!
    //! 时限：30 秒自动清除（附录 H.3），不广播、不通知、不留痕。
    //!
    //! ## 连接交接
    //!
    //! 后到的一方（B）把「自己的阶段一帧 + 自己的连接」通过通道交给先到的一方（A），
    //! 然后 B 的任务直接收工。A 醒来后同时握着两条连接，由它完成双向中继。
    //! 好处：池子里从不存放 socket，也不会出现两个任务共同操作同一条连接。

    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;

    use tokio::sync::oneshot;

    use crate::frame::{Identity, Phase1Frame, TokenHash};

    /// 后到一方交给先到一方的包裹。
    pub struct Handoff<T> {
        /// 后到方的阶段一帧（由接管方写入先到方的连接）
        pub frame: Phase1Frame,
        /// 后到方的连接
        pub payload: T,
    }

    struct Entry<T> {
        id: u64,
        identity: Identity,
        frame: Phase1Frame,
        deliver: oneshot::Sender<Handoff<T>>,
    }

    /// 等候池。
    pub struct Lobby<T> {
        /// key = 在等的人要找的目标令牌哈希（即 entry.identity.dst）
        waiters: Mutex<HashMap<TokenHash, Vec<Entry<T>>>>,
        next_id: AtomicU64,
    }

    /// 登记结果。
    pub enum Outcome<T> {
        /// 配上了。你是后到的一方：把 `Handoff{ frame: 你的帧, payload: 你的连接 }`
        /// 发进 `deliver_to_peer`，你的任务即可收工。
        Matched {
            /// 先到方的阶段一帧（由接管方负责写入先到方自己的连接）
            #[allow(dead_code)]
            peer_frame: Phase1Frame,
            deliver_to_peer: oneshot::Sender<Handoff<T>>,
        },
        /// 没人等，进池子等对端
        Waiting {
            rx: oneshot::Receiver<Handoff<T>>,
            ticket: Ticket,
        },
    }

    /// 退出池子的凭据。超时或断开时用来清理自己的登记。
    #[derive(Clone, Copy)]
    pub struct Ticket {
        id: u64,
        key: TokenHash,
    }

    impl<T> Lobby<T> {
        pub fn new() -> Self {
            Self {
                waiters: Mutex::new(HashMap::new()),
                next_id: AtomicU64::new(1),
            }
        }

        /// 登记自己；能配上就直接配对。
        pub fn register(&self, frame: Phase1Frame) -> Outcome<T> {
            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            let me = frame.identity;
            let (tx, rx) = oneshot::channel();

            let mut guard = crate::lock(&self.waiters);

            // 我要找的人，正是「回头会来找我」的那个 key
            let key = me.src;
            if let Some(entries) = guard.get_mut(&key) {
                if let Some(pos) = entries
                    .iter()
                    // 双向匹配，并额外排除自配对（纵深防御：frame 层已拒 src == dst）
                    .position(|e| {
                        me.src != me.dst && e.identity.src == me.dst && e.identity.dst == me.src
                    })
                {
                    let peer = entries.remove(pos);
                    if entries.is_empty() {
                        guard.remove(&key);
                    }
                    drop(guard);
                    return Outcome::Matched {
                        peer_frame: peer.frame,
                        deliver_to_peer: peer.deliver,
                    };
                }
            }

            guard.entry(me.dst).or_default().push(Entry {
                id,
                identity: me,
                frame,
                deliver: tx,
            });

            Outcome::Waiting {
                rx,
                ticket: Ticket { id, key: me.dst },
            }
        }

        /// 退出池子。超时 / 断开时调用。
        pub fn withdraw(&self, ticket: Ticket) {
            let mut guard = crate::lock(&self.waiters);
            if let Some(entries) = guard.get_mut(&ticket.key) {
                entries.retain(|e| e.id != ticket.id);
                if entries.is_empty() {
                    guard.remove(&ticket.key);
                }
            }
        }

        /// 清理「没人接听」的僵尸登记。返回清理条数。
        pub fn sweep(&self) -> usize {
            let mut guard = crate::lock(&self.waiters);
            let mut removed = 0usize;
            guard.retain(|_, entries| {
                let before = entries.len();
                entries.retain(|e| !e.deliver.is_closed());
                removed += before - entries.len();
                !entries.is_empty()
            });
            removed
        }

        /// 当前等候人数（诊断用，不含身份信息）。
        #[allow(dead_code)]
        pub fn waiting_count(&self) -> usize {
            crate::lock(&self.waiters).values().map(|v| v.len()).sum()
        }
    }

    impl<T> Default for Lobby<T> {
        fn default() -> Self {
            Self::new()
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::frame::TokenHash;

        fn param() -> crate::frame::ParamSet {
            crate::frame::ParamSet {
                aead: "AES-256-GCM".into(),
                hash: "SHA3-256".into(),
                key_len: 256,
                module_flags: serde_json::Map::new(),
                random_len: 256,
                random_rate: 1024,
                window: 60,
            }
        }

        fn frame(src: u8, dst: u8) -> Phase1Frame {
            Phase1Frame {
                raw: vec![0u8; 228],
                param: param(),
                identity: Identity {
                    src: TokenHash([src; 32]),
                    dst: TokenHash([dst; 32]),
                },
            }
        }

        #[test]
        fn mutual_match_pairs_both() {
            let lobby: Lobby<u32> = Lobby::new();
            let _a = lobby.register(frame(0xAA, 0xBB));
            assert_eq!(lobby.waiting_count(), 1);

            match lobby.register(frame(0xBB, 0xAA)) {
                Outcome::Matched {
                    peer_frame,
                    deliver_to_peer,
                } => {
                    assert_eq!(peer_frame.identity.src.0, [0xAA; 32]);
                    // 模拟后到方把自己交过去
                    assert!(
                        deliver_to_peer
                            .send(Handoff {
                                frame: frame(0xBB, 0xAA),
                                payload: 42
                            })
                            .is_ok(),
                        "先到方应该还在等"
                    );
                }
                _ => panic!("B 应该配上 A"),
            }
            assert_eq!(lobby.waiting_count(), 0);
        }

        #[test]
        fn same_target_without_reverse_match_does_not_pair() {
            let lobby: Lobby<u32> = Lobby::new();
            let _a = lobby.register(frame(0xAA, 0xBB));
            assert!(matches!(
                lobby.register(frame(0xCC, 0xBB)),
                Outcome::Waiting { .. }
            ));
            assert_eq!(lobby.waiting_count(), 2);
        }

        #[test]
        fn self_pairing_is_refused() {
            let lobby: Lobby<u32> = Lobby::new();
            // src == dst 的两次登记不得互相配上
            let _a = lobby.register(frame(0xAA, 0xAA));
            let b = lobby.register(frame(0xAA, 0xAA));
            assert!(
                matches!(b, Outcome::Waiting { .. }),
                "自地址登记不得与自身配对"
            );
        }

        #[test]
        fn impersonation_rejected() {
            let lobby: Lobby<u32> = Lobby::new();
            let _a = lobby.register(frame(0xAA, 0xBB));
            // X 冒充：src=X, dst=A，A 要找的是 B，双向不成立
            assert!(matches!(
                lobby.register(frame(0xCC, 0xAA)),
                Outcome::Waiting { .. }
            ));
        }

        #[test]
        fn withdraw_cleans_up() {
            let lobby: Lobby<u32> = Lobby::new();
            let ticket = match lobby.register(frame(0xAA, 0xBB)) {
                Outcome::Waiting { ticket, .. } => ticket,
                _ => unreachable!(),
            };
            assert_eq!(lobby.waiting_count(), 1);
            lobby.withdraw(ticket);
            assert_eq!(lobby.waiting_count(), 0);
        }

        #[test]
        fn sweep_removes_dropped_receivers() {
            let lobby: Lobby<u32> = Lobby::new();
            match lobby.register(frame(0xAA, 0xBB)) {
                Outcome::Waiting { rx, .. } => drop(rx),
                _ => unreachable!(),
            }
            assert_eq!(lobby.sweep(), 1);
            assert_eq!(lobby.waiting_count(), 0);
        }
    }
}

mod relay {
    //! 配对成功后的双向字节中继。
    //!
    //! 服务器只做三件事：把 A 的字节倒给 B、把 B 的字节倒给 A、计数。
    //! 不解析、不改写、不解密。

    use std::io;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

    use crate::metrics::Metrics;

    /// 中继参数。
    pub struct RelayOpts {
        pub buffer_bytes: usize,
        /// 单向读空闲超时；None = 不设
        pub idle_timeout: Option<Duration>,
        /// 单次写超时；防止对端只连不读把任务永久挂住。None = 不设。
        pub write_timeout: Option<Duration>,
    }

    /// 中继结束原因。空闲超时与正常 EOF 必须可区分，否则日志里分不清对端走了还是对端还在但一直不说话。
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum StopCause {
        /// 对端正常关闭
        Eof,
        /// 读空闲超时
        Idle,
        /// 写超时（对端只连不读）
        WriteStalled,
        /// 其他 IO 错误
        Error,
    }

    /// 单向搬运。`counter` 是跨 future 共享的累计字节数。
    async fn pipe<R, W>(
        r: &mut R,
        w: &mut W,
        opts: &RelayOpts,
        counter: &AtomicU64,
    ) -> io::Result<StopCause>
    where
        R: AsyncRead + Unpin,
        W: AsyncWrite + Unpin,
    {
        let mut buf = vec![0u8; opts.buffer_bytes.max(1024)];

        loop {
            let n = match opts.idle_timeout {
                None => r.read(&mut buf).await?,
                Some(d) => match tokio::time::timeout(d, r.read(&mut buf)).await {
                    Err(_) => return Ok(StopCause::Idle),
                    Ok(res) => res?,
                },
            };
            if n == 0 {
                return Ok(StopCause::Eof);
            }

            match opts.write_timeout {
                None => {
                    w.write_all(&buf[..n]).await?;
                    w.flush().await?;
                }
                Some(d) => {
                    let written = tokio::time::timeout(d, async {
                        w.write_all(&buf[..n]).await?;
                        w.flush().await
                    })
                    .await;
                    match written {
                        Err(_) => return Ok(StopCause::WriteStalled),
                        Ok(Err(e)) => return Err(e),
                        Ok(Ok(())) => {}
                    }
                }
            }

            counter.fetch_add(n as u64, Ordering::Relaxed);
        }
    }

    /// 双向中继一对已配对的连接。
    ///
    /// 任一侧结束即**取消**另一侧（`select!` 丢弃未完成的 future），
    /// 不等待半开连接自然超时。
    pub async fn relay_pair<A, B>(
        a: A,
        b: B,
        opts: RelayOpts,
        metrics: &Metrics,
    ) -> (u64, u64, StopCause)
    where
        A: AsyncRead + AsyncWrite + Unpin,
        B: AsyncRead + AsyncWrite + Unpin,
    {
        let (mut ar, mut aw) = tokio::io::split(a);
        let (mut br, mut bw) = tokio::io::split(b);

        let ab_bytes = AtomicU64::new(0);
        let ba_bytes = AtomicU64::new(0);

        let mut ab = Box::pin(pipe(&mut ar, &mut bw, &opts, &ab_bytes));
        let mut ba = Box::pin(pipe(&mut br, &mut aw, &opts, &ba_bytes));

        let cause = tokio::select! {
            r = &mut ab => r.unwrap_or(StopCause::Error),
            r = &mut ba => r.unwrap_or(StopCause::Error),
        };

        // 显式丢弃未完成的一侧：select! 只停止轮询，future 仍被 pin 住
        drop(ab);
        drop(ba);

        let ab_n = ab_bytes.load(Ordering::Relaxed);
        let ba_n = ba_bytes.load(Ordering::Relaxed);
        Metrics::add(&metrics.bytes_in, ab_n);
        Metrics::add(&metrics.bytes_out, ba_n);
        (ab_n, ba_n, cause)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

        fn opts() -> RelayOpts {
            RelayOpts {
                buffer_bytes: 8192,
                idle_timeout: None,
                write_timeout: Some(Duration::from_secs(5)),
            }
        }

        async fn write_and_read(
            mut from: DuplexStream,
            mut to: DuplexStream,
            payload: &'static [u8],
        ) -> Vec<u8> {
            let w = tokio::spawn(async move {
                from.write_all(payload).await.unwrap();
                from.flush().await.unwrap();
            });
            let mut got = vec![0u8; payload.len()];
            to.read_exact(&mut got).await.unwrap();
            w.await.unwrap();
            got
        }

        #[tokio::test]
        async fn relays_both_directions() {
            let m = Metrics::default();
            let (c1, s1) = tokio::io::duplex(65536);
            let (c2, s2) = tokio::io::duplex(65536);

            let h = tokio::spawn(async move { relay_pair(s1, s2, opts(), &m).await });

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

            let _ = write_and_read(c1, c2, b"").await;
            let (ab, ba, _cause) = h.await.unwrap();
            assert!(ab >= 4, "A→B 至少 4 字节，实际 {ab}");
            assert!(ba >= 4, "B→A 至少 4 字节，实际 {ba}");
        }

        #[tokio::test]
        async fn one_side_closing_cancels_the_other() {
            let m = Metrics::default();
            let (c1, s1) = tokio::io::duplex(1024);
            let (c2, s2) = tokio::io::duplex(1024);
            let o = RelayOpts {
                buffer_bytes: 1024,
                idle_timeout: None,
                write_timeout: None,
            };
            let h = tokio::spawn(async move { relay_pair(s1, s2, o, &m).await });
            drop(c1);
            let t0 = std::time::Instant::now();
            let _ = h.await.unwrap();
            assert!(
                t0.elapsed() < Duration::from_secs(2),
                "一侧关闭应立刻取消另一侧"
            );
            drop(c2);
        }

        #[tokio::test]
        async fn idle_timeout_closes_quiet_pair() {
            let m = Metrics::default();
            let (c1, s1) = tokio::io::duplex(1024);
            let (c2, s2) = tokio::io::duplex(1024);
            let o = RelayOpts {
                buffer_bytes: 1024,
                idle_timeout: Some(Duration::from_millis(120)),
                write_timeout: None,
            };
            let h = tokio::spawn(async move { relay_pair(s1, s2, o, &m).await });
            let t0 = std::time::Instant::now();
            let _ = h.await.unwrap();
            assert!(t0.elapsed() < Duration::from_secs(2));
            drop(c1);
            drop(c2);
        }
    }
}

mod config {
    //! 配置 —— 对应规范附录 B.2 的服务器三档。
    //!
    //! 三档的差别只在「存多少、等多久、放多严」，协议本身不变。
    //! 数据面永远是纯中继，服务器永远不解密。
    //!
    //! ## 隐私默认
    //!
    //! 规范第 979 行【强制】：服务器运营者**不得**记录阶段一的配对信息。
    //! 身份目录本质上是「某个令牌哈希何时出现过」的记录，即配对信息本身。
    //! 因此 `Directory.enabled` 在**所有档位**都默认关闭 —— 只有运营者显式
    //! 在配置文件里打开，才构成一个已知偏离规范意愿的扩展。

    use std::net::SocketAddr;
    use std::path::Path;

    use serde::{Deserialize, Serialize};

    /// 服务器档位（附录 B.2）。
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "kebab-case")]
    pub enum Profile {
        Beginner,
        Advanced,
        HighAnonymity,
    }

    impl Profile {
        /// 给人看的档位名（附录 B.2 的 B2-a/B2-b/B2-c）。
        ///
        /// 序列化名只有 serde 的 kebab-case 一处真值，这里不再写第二份；
        /// 由 profile_serialized_names_are_pinned 把它钉住。
        pub fn display_name(&self) -> &'static str {
            match self {
                Profile::Beginner => "B2-a 入门公益",
                Profile::Advanced => "B2-b 高级生产",
                Profile::HighAnonymity => "B2-c 高匿名",
            }
        }
    }

    /// 离线消息队列（L4 扩展）。
    #[derive(Clone, Debug, Serialize, Deserialize)]
    pub struct OfflineConfig {
        pub enabled: bool,
        pub max_payload: usize,
        pub max_per_recipient: usize,
        /// 全局收件人条目上限，防止无鉴权控制面把 HashMap 撑爆
        pub max_recipients: usize,
        pub ttl_secs: u64,
    }

    /// 群组扇出（L4 扩展）。
    #[derive(Clone, Debug, Serialize, Deserialize)]
    pub struct GroupConfig {
        pub enabled: bool,
        pub max_members: usize,
        /// 全局群组数量上限
        pub max_groups: usize,
        pub max_payload: usize,
    }

    /// 身份目录（L4 扩展）。**默认关闭**，见模块文档。
    #[derive(Clone, Debug, Serialize, Deserialize)]
    pub struct DirectoryConfig {
        pub enabled: bool,
        /// 记录保留时长（秒）
        pub ttl_secs: u64,
    }

    /// 全局配置。
    #[derive(Clone, Debug, Serialize, Deserialize)]
    pub struct Config {
        pub listen: SocketAddr,
        /// 控制面监听地址；None = 关闭。默认关闭。
        pub control_listen: Option<SocketAddr>,
        pub profile: Profile,
        pub handshake_timeout_secs: u64,
        pub lobby_timeout_secs: u64,
        pub idle_timeout_secs: u64,
        pub max_connections: usize,
        pub max_per_ip: usize,
        pub buffer_bytes: usize,
        pub offline: OfflineConfig,
        pub group: GroupConfig,
        pub directory: DirectoryConfig,
    }

    impl Default for Config {
        fn default() -> Self {
            Self::for_profile(Profile::Beginner)
        }
    }

    impl Config {
        /// 按档位给出一套默认值。
        pub fn for_profile(p: Profile) -> Self {
            let (max_conn, per_ip, idle, offline, group) = match p {
                Profile::Beginner => (
                    2_000,
                    32,
                    0,
                    OfflineConfig {
                        enabled: true,
                        max_payload: 64 * 1024,
                        max_per_recipient: 20,
                        max_recipients: 10_000,
                        ttl_secs: 86_400,
                    },
                    GroupConfig {
                        enabled: true,
                        max_members: 32,
                        max_groups: 10_000,
                        max_payload: 64 * 1024,
                    },
                ),
                Profile::Advanced => (
                    20_000,
                    128,
                    0,
                    OfflineConfig {
                        enabled: true,
                        max_payload: 256 * 1024,
                        max_per_recipient: 100,
                        max_recipients: 100_000,
                        ttl_secs: 604_800,
                    },
                    GroupConfig {
                        enabled: true,
                        max_members: 256,
                        max_groups: 50_000,
                        max_payload: 256 * 1024,
                    },
                ),
                Profile::HighAnonymity => (
                    500,
                    4,
                    300,
                    OfflineConfig {
                        enabled: false,
                        max_payload: 16 * 1024,
                        max_per_recipient: 0,
                        max_recipients: 0,
                        ttl_secs: 0,
                    },
                    GroupConfig {
                        enabled: false,
                        max_members: 8,
                        max_groups: 0,
                        max_payload: 16 * 1024,
                    },
                ),
            };

            Self {
                listen: "0.0.0.0:7443".parse().expect("默认监听地址合法"),
                // 控制面默认关闭：未显式 --control 不监听任何管理端口
                control_listen: None,
                profile: p,
                handshake_timeout_secs: 15,
                lobby_timeout_secs: 30,
                idle_timeout_secs: idle,
                max_connections: max_conn,
                max_per_ip: per_ip,
                buffer_bytes: 32 * 1024,
                offline,
                group,
                // 所有档位默认关闭：见模块文档
                directory: DirectoryConfig {
                    enabled: false,
                    ttl_secs: 3_600,
                },
            }
        }

        /// 启动前校验：把「运行时静默改写成合法值」换成「启动时拒绝」。
        pub fn validate(&self) -> Result<(), String> {
            // 无条件生效的项
            if self.handshake_timeout_secs == 0 {
                return Err("handshake_timeout_secs must be greater than zero".into());
            }
            if self.lobby_timeout_secs == 0 {
                return Err("lobby_timeout_secs must be greater than zero".into());
            }
            if self.max_connections == 0 {
                return Err("max_connections must be greater than zero".into());
            }
            if self.buffer_bytes == 0 {
                return Err("buffer_bytes must be greater than zero".into());
            }

            // 条件生效的项：功能关着时，它自己的参数取 0 是正常的
            // （例如高匿名档下 offline 是关的，ttl_secs 本来就是 0）。
            if self.offline.enabled {
                if self.offline.ttl_secs == 0 {
                    return Err(
                        "offline.ttl_secs must be greater than zero when offline is enabled".into(),
                    );
                }
                if self.offline.max_per_recipient == 0 {
                    return Err(
                        "offline.max_per_recipient must be greater than zero when offline is enabled"
                            .into(),
                    );
                }
                if self.offline.max_recipients == 0 {
                    return Err(
                        "offline.max_recipients must be greater than zero when offline is enabled"
                            .into(),
                    );
                }
            }
            if self.group.enabled && self.group.max_groups == 0 {
                return Err(
                    "group.max_groups must be greater than zero when group is enabled".into(),
                );
            }
            if self.directory.enabled && self.directory.ttl_secs == 0 {
                return Err(
                    "directory.ttl_secs must be greater than zero when directory is enabled".into(),
                );
            }
            Ok(())
        }

        /// 从 JSON 文件读取。
        ///
        /// 文件不存在时**返回错误**，不静默降级 —— 否则 `--config` 路径写错会让
        /// 高匿名档静默变成 beginner 档（离线存储开、目录开）。
        pub fn load(path: &Path) -> std::io::Result<Self> {
            let bytes = std::fs::read(path).map_err(|e| {
                std::io::Error::new(
                    e.kind(),
                    format!("读取配置文件 {} 失败: {e}", path.display()),
                )
            })?;
            serde_json::from_slice(&bytes).map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("配置文件 {} 解析失败: {e}", path.display()),
                )
            })
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// 零值必须让进程启动失败，而不是在运行期被静默改写成 1。
        #[test]
        fn validate_rejects_zero_durations() {
            let mut c = Config::for_profile(Profile::Beginner);
            c.handshake_timeout_secs = 0;
            assert!(c.validate().is_err(), "零超时必须被拒绝");
        }

        #[test]
        fn validate_rejects_zero_limits() {
            let mut c = Config::for_profile(Profile::Beginner);
            c.max_connections = 0;
            assert!(c.validate().is_err(), "零连接上限必须被拒绝");
        }

        /// 档位的序列化名只有 serde 一处真值，这个测试把它钉住。
        #[test]
        fn profile_serialized_names_are_pinned() {
            let cases = [
                (Profile::Beginner, "\"beginner\""),
                (Profile::Advanced, "\"advanced\""),
                (Profile::HighAnonymity, "\"high-anonymity\""),
            ];
            for (p, want) in cases {
                assert_eq!(serde_json::to_string(&p).unwrap(), want);
            }
        }

        #[test]
        fn validate_accepts_every_shipped_profile() {
            for p in [Profile::Beginner, Profile::Advanced, Profile::HighAnonymity] {
                Config::for_profile(p)
                    .validate()
                    .unwrap_or_else(|e| panic!("{p:?} 配置应可启动: {e}"));
            }
        }

        #[test]
        fn directory_is_off_in_every_profile() {
            for p in [Profile::Beginner, Profile::Advanced, Profile::HighAnonymity] {
                assert!(
                    !Config::for_profile(p).directory.enabled,
                    "身份目录必须在所有档位默认关闭（规范第 979 行）"
                );
            }
        }

        #[test]
        fn control_plane_is_off_by_default() {
            for p in [Profile::Beginner, Profile::Advanced, Profile::HighAnonymity] {
                assert!(
                    Config::for_profile(p).control_listen.is_none(),
                    "控制面必须默认关闭"
                );
            }
        }

        #[test]
        fn high_anonymity_disables_storage_features() {
            let c = Config::for_profile(Profile::HighAnonymity);
            assert!(!c.offline.enabled);
            assert!(!c.group.enabled);
            assert!(!c.directory.enabled);
            assert!(c.idle_timeout_secs > 0);
        }

        #[test]
        fn beginner_enables_relay_storage_but_not_directory() {
            let c = Config::for_profile(Profile::Beginner);
            assert!(c.offline.enabled && c.group.enabled);
            assert!(!c.directory.enabled);
        }

        #[test]
        fn missing_config_file_is_an_error_not_a_fallback() {
            let r = Config::load(Path::new("/nonexistent/zt.toml"));
            assert!(r.is_err(), "配置文件不存在必须报错退出，不能静默降级");
        }
    }
}

mod l4 {
    //! L4 扩展：离线消息 / 群组 / 身份目录。
    //!
    //! 这三项**不在 ZT-DSP 1.0.1 规范内**。它们走独立控制面（JSON Lines over TCP），
    //! 因为附录 F 的线上帧没有帧类型字段，往数据面插帧会破坏客户端互操作。
    //!
    //! payload 一律是不透明字节：服务器不解密、不解析。

    use std::collections::{HashMap, HashSet, VecDeque};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use crate::config::{DirectoryConfig, GroupConfig, OfflineConfig};
    use crate::frame::TokenHash;

    /// L4 操作错误。
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum L4Error {
        Disabled,
        PayloadTooLarge { max: usize },
        NotFound,
        AlreadyExists,
        GroupFull { max: usize },
        GroupLimit { max: usize },
        RecipientLimit { max: usize },
    }

    impl L4Error {
        /// 稳定的机器可读码。文案可改、可本地化，**码不可改**：机器客户端只认它。
        pub fn code(&self) -> &'static str {
            match self {
                Self::Disabled => "disabled",
                Self::PayloadTooLarge { .. } => "payload_too_large",
                Self::NotFound => "not_found",
                Self::AlreadyExists => "already_exists",
                Self::GroupFull { .. } => "group_full",
                Self::GroupLimit { .. } => "group_limit",
                Self::RecipientLimit { .. } => "recipient_limit",
            }
        }
    }

    impl std::error::Error for L4Error {}

    impl std::fmt::Display for L4Error {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                L4Error::Disabled => write!(f, "该功能在当前档位被关闭"),
                L4Error::PayloadTooLarge { max } => write!(f, "载荷超过上限 {max} 字节"),
                L4Error::NotFound => write!(f, "未找到"),
                L4Error::AlreadyExists => write!(f, "已存在"),
                L4Error::GroupFull { max } => write!(f, "群成员已达上限 {max}"),
                L4Error::GroupLimit { max } => write!(f, "群组数量已达上限 {max}"),
                L4Error::RecipientLimit { max } => write!(f, "离线收件人数量已达上限 {max}"),
            }
        }
    }

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

    impl OfflineStore {
        pub fn new(cfg: OfflineConfig) -> Self {
            Self {
                cfg,
                inner: Mutex::new(HashMap::new()),
            }
        }

        /// 投递一条离线消息，返回该收件人当前积压条数。
        pub fn push(&self, dst: TokenHash, payload: Vec<u8>) -> Result<usize, L4Error> {
            if !self.cfg.enabled || self.cfg.max_per_recipient == 0 {
                return Err(L4Error::Disabled);
            }
            if payload.len() > self.cfg.max_payload {
                return Err(L4Error::PayloadTooLarge {
                    max: self.cfg.max_payload,
                });
            }
            let mut guard = crate::lock(&self.inner);

            if !guard.contains_key(&dst) && guard.len() >= self.cfg.max_recipients {
                return Err(L4Error::RecipientLimit {
                    max: self.cfg.max_recipients,
                });
            }

            let q = guard.entry(dst).or_default();
            if q.len() >= self.cfg.max_per_recipient {
                q.pop_front();
            }
            q.push_back(Stored {
                payload,
                stored_at: Instant::now(),
            });
            Ok(q.len())
        }

        /// 取走某收件人的至多 max 条消息。
        pub fn pull(&self, dst: TokenHash, max: usize) -> Result<Vec<Vec<u8>>, L4Error> {
            if !self.cfg.enabled {
                return Err(L4Error::Disabled);
            }
            let mut guard = crate::lock(&self.inner);
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

        /// 清理过期消息，返回清理条数。
        pub fn sweep(&self) -> usize {
            if !self.cfg.enabled {
                return 0;
            }
            let ttl = Duration::from_secs(self.cfg.ttl_secs);
            let mut guard = crate::lock(&self.inner);
            let mut removed = 0usize;
            guard.retain(|_, q| {
                let before = q.len();
                q.retain(|s| s.stored_at.elapsed() < ttl);
                removed += before - q.len();
                !q.is_empty()
            });
            removed
        }

        pub fn depth(&self) -> usize {
            crate::lock(&self.inner).values().map(|q| q.len()).sum()
        }
    }

    // ───────────────────────── 群组 ─────────────────────────

    struct Broadcast {
        payload: Vec<u8>,
        cursor: u64,
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

    /// 群组：成员名单 + 追加日志，成员按游标拉取。
    pub struct Groups {
        cfg: GroupConfig,
        inner: Mutex<HashMap<String, GroupState>>,
    }

    impl Groups {
        pub fn new(cfg: GroupConfig) -> Self {
            Self {
                cfg,
                inner: Mutex::new(HashMap::new()),
            }
        }

        /// 创建群组。同名群组已存在时返回 `AlreadyExists`，不静默复用。
        pub fn create(&self, name: &str, owner: TokenHash) -> Result<(), L4Error> {
            if !self.cfg.enabled {
                return Err(L4Error::Disabled);
            }
            if name.is_empty() {
                return Err(L4Error::NotFound);
            }
            let mut guard = crate::lock(&self.inner);
            if guard.contains_key(name) {
                return Err(L4Error::AlreadyExists);
            }
            if guard.len() >= self.cfg.max_groups {
                return Err(L4Error::GroupLimit {
                    max: self.cfg.max_groups,
                });
            }
            let mut g = GroupState::new();
            g.members.insert(owner);
            g.cursors.insert(owner, 0);
            guard.insert(name.to_string(), g);
            Ok(())
        }

        pub fn join(&self, name: &str, member: TokenHash) -> Result<usize, L4Error> {
            if !self.cfg.enabled {
                return Err(L4Error::Disabled);
            }
            let mut guard = crate::lock(&self.inner);
            let Some(g) = guard.get_mut(name) else {
                return Err(L4Error::NotFound);
            };
            if !g.members.contains(&member) && g.members.len() >= self.cfg.max_members {
                return Err(L4Error::GroupFull {
                    max: self.cfg.max_members,
                });
            }
            g.members.insert(member);
            g.cursors.entry(member).or_insert(0);
            Ok(g.members.len())
        }

        pub fn send(
            &self,
            name: &str,
            sender: TokenHash,
            payload: Vec<u8>,
        ) -> Result<u64, L4Error> {
            if !self.cfg.enabled {
                return Err(L4Error::Disabled);
            }
            if payload.len() > self.cfg.max_payload {
                return Err(L4Error::PayloadTooLarge {
                    max: self.cfg.max_payload,
                });
            }
            let mut guard = crate::lock(&self.inner);
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

        pub fn pull(&self, name: &str, member: TokenHash) -> Result<Vec<(u64, Vec<u8>)>, L4Error> {
            if !self.cfg.enabled {
                return Err(L4Error::Disabled);
            }
            let mut guard = crate::lock(&self.inner);
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
    }

    // ───────────────────────── 身份目录 ─────────────────────────

    /// 身份目录：只回答「这个令牌哈希最近活跃过吗」。
    ///
    /// 规范第 979 行禁止服务器记录配对信息，而本结构正是这样一份记录。
    /// 因此它在所有档位默认关闭；只有运营者显式开启才存在。
    /// 访问控制由控制面的身份校验负责，本类型不再接受调用方自行声明的开关。
    pub struct Directory {
        cfg: DirectoryConfig,
        seen: Mutex<HashMap<TokenHash, Instant>>,
    }

    impl Directory {
        pub fn new(cfg: DirectoryConfig) -> Self {
            Self {
                cfg,
                seen: Mutex::new(HashMap::new()),
            }
        }

        pub fn enabled(&self) -> bool {
            self.cfg.enabled
        }

        /// 记录一次活跃。
        pub fn touch(&self, hash: TokenHash) {
            if !self.cfg.enabled {
                return;
            }
            let mut guard = crate::lock(&self.seen);
            guard.insert(hash, Instant::now());
        }

        /// 查询。返回 (是否活跃, 最近活跃距今的**粗化**秒数)。
        pub fn lookup(&self, hash: TokenHash) -> Result<(bool, Option<u64>), L4Error> {
            if !self.cfg.enabled {
                return Err(L4Error::Disabled);
            }
            let guard = crate::lock(&self.seen);
            match guard.get(&hash) {
                None => Ok((false, None)),
                Some(t) => Ok((true, Some(t.elapsed().as_secs() / 60 * 60))),
            }
        }

        /// 按配置的 TTL 清理。
        pub fn sweep(&self) -> usize {
            if !self.cfg.enabled {
                return 0;
            }
            let ttl = Duration::from_secs(self.cfg.ttl_secs);
            let mut guard = crate::lock(&self.seen);
            let before = guard.len();
            guard.retain(|_, t| t.elapsed() < ttl);
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
            OfflineConfig {
                enabled: true,
                max_payload: 1024,
                max_per_recipient: 3,
                max_recipients: 4,
                ttl_secs: 3600,
            }
        }

        fn group_cfg() -> GroupConfig {
            GroupConfig {
                enabled: true,
                max_members: 8,
                max_groups: 4,
                max_payload: 1024,
            }
        }

        #[test]
        fn offline_fifo_eviction() {
            let s = OfflineStore::new(offline_cfg());
            for i in 0..3u8 {
                s.push(h(1), vec![i]).unwrap();
            }
            assert_eq!(s.push(h(1), vec![9]).unwrap(), 3);
            let items = s.pull(h(1), 10).unwrap();
            assert_eq!(items.len(), 3);
            assert_eq!(items[0], vec![1]);
            assert_eq!(items[2], vec![9]);
        }

        #[test]
        fn offline_rejects_oversize() {
            let s = OfflineStore::new(offline_cfg());
            assert!(matches!(
                s.push(h(1), vec![0u8; 2048]),
                Err(L4Error::PayloadTooLarge { .. })
            ));
        }

        #[test]
        fn offline_recipient_cap_is_enforced() {
            let s = OfflineStore::new(offline_cfg());
            for i in 0..4u8 {
                s.push(h(i), vec![i]).unwrap();
            }
            assert!(matches!(
                s.push(h(9), vec![9]),
                Err(L4Error::RecipientLimit { max: 4 })
            ));
        }

        #[test]
        fn offline_zero_quota_means_disabled() {
            let mut c = offline_cfg();
            c.max_per_recipient = 0;
            let s = OfflineStore::new(c);
            assert!(matches!(s.push(h(1), vec![1]), Err(L4Error::Disabled)));
        }

        #[test]
        fn group_create_is_not_idempotent() {
            let g = Groups::new(group_cfg());
            g.create("room", h(1)).unwrap();
            assert!(matches!(
                g.create("room", h(1)),
                Err(L4Error::AlreadyExists)
            ));
        }

        #[test]
        fn group_cursor_advances() {
            let g = Groups::new(group_cfg());
            g.create("room", h(1)).unwrap();
            g.join("room", h(2)).unwrap();
            g.send("room", h(1), b"hello".to_vec()).unwrap();
            let got = g.pull("room", h(2)).unwrap();
            assert_eq!(got.len(), 1);
            assert_eq!(got[0].1, b"hello");
            assert!(g.pull("room", h(2)).unwrap().is_empty());
        }

        #[test]
        fn group_rejects_non_member() {
            let g = Groups::new(group_cfg());
            g.create("room", h(1)).unwrap();
            assert!(matches!(
                g.send("room", h(3), b"x".to_vec()),
                Err(L4Error::NotFound)
            ));
        }

        #[test]
        fn group_count_cap_is_enforced() {
            let g = Groups::new(group_cfg());
            for i in 0..4u8 {
                g.create(&format!("g{i}"), h(1)).unwrap();
            }
            assert!(matches!(
                g.create("g9", h(1)),
                Err(L4Error::GroupLimit { max: 4 })
            ));
        }

        fn dir_cfg(enabled: bool) -> DirectoryConfig {
            DirectoryConfig {
                enabled,
                ttl_secs: 3600,
            }
        }

        #[test]
        fn directory_disabled_by_default_refuses() {
            let d = Directory::new(dir_cfg(false));
            assert!(matches!(d.lookup(h(7)), Err(L4Error::Disabled)));
        }

        #[test]
        fn directory_coarsens_time() {
            let d = Directory::new(dir_cfg(true));
            d.touch(h(7));
            let (alive, ago) = d.lookup(h(7)).unwrap();
            assert!(alive);
            // 刚写入的记录必须落在第一个整分钟桶里；这同时排除「没做粗化」的实现
            let ago = ago.expect("刚写入的记录应有时间戳");
            assert!(ago < 60, "刚写入的记录粗化后应小于 60 秒，实际 {ago}");
            assert_eq!(ago % 60, 0, "粗化结果必须是整分钟");
        }

        #[test]
        fn disabled_features_refuse() {
            let s = OfflineStore::new(OfflineConfig {
                enabled: false,
                max_payload: 1,
                max_per_recipient: 1,
                max_recipients: 1,
                ttl_secs: 1,
            });
            assert!(matches!(s.push(h(1), vec![1]), Err(L4Error::Disabled)));
        }
    }
}

mod control {
    //! L4 控制面：JSON Lines over TCP。
    //!
    //! 与数据面完全隔离，不碰 ZT-DSP 线上帧。**默认关闭**，且启用时强制要求口令。
    //!
    //! 访问控制只有口令校验，没有按令牌的身份绑定 —— 因此本控制面按设计仅供
    //! 运营者自己使用（运维脚本、管理端的账号体系在外层）。

    use std::net::SocketAddr;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use serde::{Deserialize, Serialize};
    use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::{TcpListener, TcpStream};
    use tracing::{debug, info, warn};

    use crate::l4::{Directory, Groups, L4Error, OfflineStore};
    use crate::metrics::Metrics;
    use crate::util::{from_hex, parse_token_hash, to_hex};

    /// 单行请求上限
    pub const MAX_LINE_BYTES: usize = 2 * 1024 * 1024;
    /// 控制面并发连接上限
    pub const MAX_CONTROL_CONNS: usize = 16;
    /// 读完一行请求的空闲超时
    pub const READ_TIMEOUT: Duration = Duration::from_secs(30);
    /// 单连接允许的鉴权失败次数
    pub const MAX_AUTH_FAILURES: u32 = 5;

    /// 控制面共享状态。
    pub struct ControlState {
        pub offline: Arc<OfflineStore>,
        pub groups: Arc<Groups>,
        pub directory: Arc<Directory>,
        pub metrics: Arc<Metrics>,
        /// 必填：没有口令就不启动控制面
        pub token: String,
    }

    #[derive(Deserialize)]
    #[serde(tag = "op", rename_all = "snake_case")]
    #[serde(deny_unknown_fields)]
    enum Req {
        Auth {
            token: String,
        },
        Ping,
        OfflinePush {
            dst: String,
            payload: String,
        },
        OfflinePull {
            dst: String,
            max: Option<usize>,
        },
        GroupCreate {
            name: String,
            owner: String,
        },
        GroupJoin {
            name: String,
            member: String,
        },
        GroupSend {
            name: String,
            sender: String,
            payload: String,
        },
        GroupPull {
            name: String,
            member: String,
        },
        DirLookup {
            hash: String,
        },
        Stats,
    }

    #[derive(Serialize)]
    struct Resp {
        ok: bool,
        /// 稳定的机器可读码。机器客户端只认这个；error 字段是给人看的，可本地化。
        #[serde(skip_serializing_if = "Option::is_none")]
        error_code: Option<&'static str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<serde_json::Value>,
    }

    impl Resp {
        fn ok(data: serde_json::Value) -> Self {
            Self {
                ok: true,
                error_code: None,
                error: None,
                data: Some(data),
            }
        }
        /// 请求本身不合法（形状、字段类型），无领域错误码。
        fn err(e: impl std::fmt::Display) -> Self {
            Self {
                ok: false,
                error_code: Some("bad_request"),
                error: Some(e.to_string()),
                data: None,
            }
        }
    }

    fn e(err: L4Error) -> Resp {
        Resp {
            ok: false,
            error_code: Some(err.code()),
            error: Some(err.to_string()),
            data: None,
        }
    }

    /// 常数时间比较，避免按字节短路泄露口令前缀。
    fn ct_eq(a: &[u8], b: &[u8]) -> bool {
        // 长度不同也走完整轮比较：用耗时去区分长度差本身就是侧信道。
        let n = a.len().max(b.len());
        let mut acc: u8 = 0;
        for i in 0..n {
            let x = a.get(i).copied().unwrap_or(0);
            let y = b.get(i).copied().unwrap_or(0);
            acc |= x ^ y;
        }
        acc |= (a.len() != b.len()) as u8;
        acc == 0
    }

    /// 读一行，超过上限即报错（不无限增长缓冲区）。
    async fn read_line_limited<R: AsyncBufRead + Unpin>(
        r: &mut R,
        max: usize,
        out: &mut Vec<u8>,
    ) -> std::io::Result<Option<usize>> {
        out.clear();
        loop {
            // 只 fill_buf 一次：两次调用之间缓冲区可能被改写，chunk_len 就会失配。
            let (chunk_len, hit_newline) = {
                let available = r.fill_buf().await?;
                if available.is_empty() {
                    if out.is_empty() {
                        return Ok(None);
                    }
                    return Ok(Some(out.len()));
                }
                match available.iter().position(|&b| b == b'\n') {
                    Some(p) => (p + 1, true),
                    None => (available.len(), false),
                }
            };

            if out.len() + chunk_len > max {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "control line too long",
                ));
            }
            {
                let available = r.fill_buf().await?;
                out.extend_from_slice(&available[..chunk_len]);
            }
            r.consume(chunk_len);
            if hit_newline {
                return Ok(Some(out.len()));
            }
        }
    }

    /// 启动控制面。
    pub async fn serve(addr: SocketAddr, state: Arc<ControlState>) -> std::io::Result<()> {
        let listener = TcpListener::bind(addr).await?;
        info!(%addr, "控制面已监听（需要口令）");
        let live = Arc::new(AtomicUsize::new(0));

        loop {
            let (stream, peer) = match listener.accept().await {
                Ok(v) => v,
                Err(err) => {
                    warn!(error = %err, "控制面 accept 失败");
                    continue;
                }
            };

            if live.load(Ordering::Relaxed) >= MAX_CONTROL_CONNS {
                debug!(%peer, "控制面连接数已满，拒绝");
                drop(stream);
                continue;
            }
            live.fetch_add(1, Ordering::Relaxed);

            let st = Arc::clone(&state);
            let live2 = Arc::clone(&live);
            tokio::spawn(async move {
                if let Err(err) = handle(stream, st).await {
                    debug!(%peer, error = %err, "控制面连接结束");
                }
                live2.fetch_sub(1, Ordering::Relaxed);
            });
        }
    }

    async fn handle(stream: TcpStream, state: Arc<ControlState>) -> std::io::Result<()> {
        stream.set_nodelay(true).ok();
        let (rd, mut wr) = stream.into_split();
        let mut reader = BufReader::new(rd);
        let mut line = Vec::new();
        let mut authed = false;
        let mut failures: u32 = 0;

        loop {
            let read = tokio::time::timeout(
                READ_TIMEOUT,
                read_line_limited(&mut reader, MAX_LINE_BYTES, &mut line),
            )
            .await;

            let n = match read {
                Err(_) => return Ok(()),
                Ok(Err(err)) => {
                    let r = Resp::err(format!("请求行非法: {err}"));
                    write_resp(&mut wr, &r).await?;
                    return Ok(());
                }
                Ok(Ok(None)) => return Ok(()),
                Ok(Ok(Some(n))) => n,
            };
            if n == 0 {
                continue;
            }

            let text = match std::str::from_utf8(&line) {
                Ok(t) => t.trim(),
                Err(_) => {
                    write_resp(&mut wr, &Resp::err("请求不是合法 UTF-8")).await?;
                    continue;
                }
            };
            if text.is_empty() {
                continue;
            }

            let resp = match serde_json::from_str::<Req>(text) {
                Err(err) => Resp::err(format!("请求解析失败: {err}")),
                Ok(Req::Auth { token }) => {
                    if ct_eq(token.as_bytes(), state.token.as_bytes()) {
                        authed = true;
                        Resp::ok(serde_json::json!({"authed": true}))
                    } else {
                        failures += 1;
                        if failures >= MAX_AUTH_FAILURES {
                            write_resp(&mut wr, &Resp::err("鉴权失败次数过多")).await?;
                            return Ok(());
                        }
                        Resp::err("鉴权失败")
                    }
                }
                Ok(_) if !authed => Resp::err("未鉴权：请先发送 auth 请求"),
                Ok(req) => dispatch(req, &state),
            };

            write_resp(&mut wr, &resp).await?;
        }
    }

    async fn write_resp<W: tokio::io::AsyncWrite + Unpin>(
        w: &mut W,
        r: &Resp,
    ) -> std::io::Result<()> {
        // 序列化失败不能静默替换成假响应：那会把内部错误伪装成服务端拒绝。
        let mut out = match serde_json::to_vec(r) {
            Ok(v) => v,
            Err(err) => {
                warn!(error = %err, "控制面响应序列化失败");
                br#"{"ok":false,"error_code":"internal"}"#.to_vec()
            }
        };
        out.push(b'\n');
        w.write_all(&out).await?;
        w.flush().await
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

            Req::GroupSend {
                name,
                sender,
                payload,
            } => {
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
                match state.directory.lookup(h) {
                    Ok((alive, ago)) => {
                        Resp::ok(serde_json::json!({"alive": alive, "ago_secs": ago}))
                    }
                    Err(err) => e(err),
                }
            }

            Req::Stats => Resp::ok(serde_json::json!({
                "accepted": Metrics::get(&state.metrics.accepted),
                "paired": Metrics::get(&state.metrics.paired),
                "active_pairs": Metrics::get(&state.metrics.active_pairs),
                "active_conns": Metrics::get(&state.metrics.active_conns),
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

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn ct_eq_matches_normal_equality() {
            assert!(ct_eq(b"abc", b"abc"));
            assert!(!ct_eq(b"abc", b"abd"));
            assert!(!ct_eq(b"abc", b"abcd"));
            assert!(!ct_eq(b"", b"a"));
        }

        #[tokio::test]
        async fn read_line_limited_rejects_overlong_line() {
            let data = vec![b'a'; 64];
            let mut r = BufReader::new(std::io::Cursor::new(data));
            let mut out = Vec::new();
            let e = read_line_limited(&mut r, 16, &mut out).await;
            assert!(e.is_err(), "超长行必须报错而不是无限缓冲");
        }

        #[tokio::test]
        async fn read_line_limited_reads_line() {
            let data = b"hello\nworld\n".to_vec();
            let mut r = BufReader::new(std::io::Cursor::new(data));
            let mut out = Vec::new();
            let n = read_line_limited(&mut r, 64, &mut out)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(n, 6);
            assert_eq!(out, b"hello\n");
        }
    }
}

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
#[command(
    name = "zt-dsp-server",
    version = version::IMPLEMENTATION_ID,
    about = "ZT-DSP 1.0.1 server"
)]
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
        let mut g = crate::lock(self.map);
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

    eprintln!("ZT-DSP · {}", version::format_version());
    debug!(
        role = version::ROLE,
        prefix = version::ROLE_PREFIX,
        code = version::CODE_VERSION,
        doc = version::FOLLOWS_DOC,
        doc_path = version::FOLLOWS_DOC_PATH,
        applies_to = ?version::APPLIES_TO_DOC_VERSIONS,
        clients = ?version::CLIENT_VERSIONS,
        servers = ?version::SERVER_VERSIONS,
        "version contract"
    );
    info!(
        profile = cfg.profile.display_name(),
        listen = %cfg.listen,
        control = ?cfg.control_listen,
        directory = cfg.directory.enabled,
        "starting ZT-DSP relay (no decryption)"
    );
    if cfg.directory.enabled {
        warn!(
            "identity directory enabled: this is pairing metadata, which the specification forbids"
        );
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

    // 配置不合法就不进入监听：宁可启动失败，也不要在运行期悄悄改写调用方给的值
    if let Err(reason) = cfg.validate() {
        return Err(format!("配置校验失败: {reason}").into());
    }

    let listener = TcpListener::bind(cfg.listen).await?;
    info!(addr = %cfg.listen, "data plane listening");

    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(v) => v,
            Err(err) => {
                // 不退避的话，EMFILE 之类的瞬时错误会变成满速重试
                warn!(error = %err, "accept failed, backing off");
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
        };

        if shared.metrics.active_conns.load(Ordering::Relaxed) as usize
            >= shared.cfg.max_connections
        {
            Metrics::inc(&shared.metrics.rejected_silent);
            // 过载也不得直接 FIN：与其它拒绝路径保持同样的静默观感与时长。
            // 但挂起任务数必须有上限，否则洪泛时任务数会失控。
            let d = Duration::from_secs(shared.cfg.handshake_timeout_secs);
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
        let mut map = crate::lock(&shared.per_ip);
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

    let deadline = Instant::now() + Duration::from_secs(shared.cfg.handshake_timeout_secs);

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
            let lobby_timeout = Duration::from_secs(shared.cfg.lobby_timeout_secs);
            match timeout(lobby_timeout, rx).await {
                Ok(Ok(handoff)) => {
                    Metrics::inc(&shared.metrics.paired);
                    shared.metrics.active_pairs.fetch_add(1, Ordering::Relaxed);
                    if shared.directory.enabled() {
                        shared.directory.touch(identity.src);
                        shared.directory.touch(identity.dst);
                    }

                    let mut peer_stream = handoff.payload;
                    let phase1_replay = Duration::from_secs(shared.cfg.handshake_timeout_secs);
                    let written = async {
                        write_frame(&mut peer_stream, &my_frame, phase1_replay).await?;
                        write_frame(&mut stream, &handoff.frame, phase1_replay).await?;
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
                    let (ab, ba, cause) =
                        relay_pair(stream, peer_stream, opts, &shared.metrics).await;
                    debug!(peer = %peer, ab, ba, ?cause, "relay finished");
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
        Duration::from_secs(shared.cfg.handshake_timeout_secs),
    )
    .await;
}

async fn write_frame(s: &mut TcpStream, f: &Phase1Frame, timeout: Duration) -> std::io::Result<()> {
    tokio::time::timeout(timeout, async {
        s.write_all(&f.raw).await?;
        s.flush().await
    })
    .await
    .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "phase1 replay timed out"))?
}
