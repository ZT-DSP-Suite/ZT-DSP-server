// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 ZT-DSP Suite contributors

//! 配置 —— 对应规范附录 B.2 的服务器三档。
//!
//! 三档的差别只在「存多少、等多久、放多严」，协议本身不变。
//! 数据面永远是纯中继，服务器永远不解密。

use std::net::SocketAddr;

use serde::{Deserialize, Serialize};

/// 服务器档位（附录 B.2）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Profile {
    /// B2-a 入门公益：小规模、开源
    Beginner,
    /// B2-b 高级生产：商业部署
    Advanced,
    /// B2-c 高匿名：调查记者
    HighAnonymity,
}

impl Profile {
    pub fn as_str(&self) -> &'static str {
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
    /// 单条载荷上限（字节）
    pub max_payload: usize,
    /// 单个收件人积压上限（条）
    pub max_per_recipient: usize,
    /// 保留时长（秒）
    pub ttl_secs: u64,
}

/// 群组扇出（L4 扩展）。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GroupConfig {
    pub enabled: bool,
    /// 单群成员上限
    pub max_members: usize,
    /// 单条广播载荷上限
    pub max_payload: usize,
}

/// 身份目录（L4 扩展）。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DirectoryConfig {
    pub enabled: bool,
    /// 是否允许查询任意哈希（为了匿名性默认关闭，只允许自己查自己）
    pub allow_arbitrary_lookup: bool,
}

/// 全局配置。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    /// 数据面监听地址（ZT-DSP 协议）
    pub listen: SocketAddr,
    /// 控制面监听地址（L4 JSON Lines）；None = 不启用
    pub control_listen: Option<SocketAddr>,
    pub profile: Profile,
    /// 握手超时（附录 H.3 默认 15s），整帧共享
    pub handshake_timeout_secs: u64,
    /// 等候池超时（附录 H.3 默认 30s）
    pub lobby_timeout_secs: u64,
    /// 配对后空闲超时；0 = 不超时
    pub idle_timeout_secs: u64,
    /// 并发连接上限
    pub max_connections: usize,
    /// 单 IP 并发上限（0 = 不限）
    pub max_per_ip: usize,
    /// 中继缓冲区大小
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
        let (max_conn, per_ip, idle, offline, group, dir) = match p {
            Profile::Beginner => (
                2_000,
                32,
                0,
                OfflineConfig { enabled: true, max_payload: 64 * 1024, max_per_recipient: 20, ttl_secs: 86_400 },
                GroupConfig { enabled: true, max_members: 32, max_payload: 64 * 1024 },
                DirectoryConfig { enabled: true, allow_arbitrary_lookup: false },
            ),
            Profile::Advanced => (
                20_000,
                128,
                0,
                OfflineConfig { enabled: true, max_payload: 256 * 1024, max_per_recipient: 100, ttl_secs: 604_800 },
                GroupConfig { enabled: true, max_members: 256, max_payload: 256 * 1024 },
                DirectoryConfig { enabled: true, allow_arbitrary_lookup: false },
            ),
            Profile::HighAnonymity => (
                500,
                4,
                300,
                OfflineConfig { enabled: false, max_payload: 16 * 1024, max_per_recipient: 0, ttl_secs: 0 },
                GroupConfig { enabled: false, max_members: 8, max_payload: 16 * 1024 },
                DirectoryConfig { enabled: false, allow_arbitrary_lookup: false },
            ),
        };

        Self {
            listen: "0.0.0.0:7443".parse().expect("默认监听地址合法"),
            control_listen: Some("127.0.0.1:7444".parse().expect("默认控制面地址合法")),
            profile: p,
            handshake_timeout_secs: 15,
            lobby_timeout_secs: 30,
            idle_timeout_secs: idle,
            max_connections: max_conn,
            max_per_ip: per_ip,
            buffer_bytes: 32 * 1024,
            offline,
            group,
            directory: dir,
        }
    }

    /// 从 JSON 文件读；文件不存在则返回按档位的默认值。
    pub fn load(path: &std::path::Path) -> std::io::Result<Self> {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, format!("配置解析失败: {e}"))
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn high_anonymity_disables_storage_features() {
        let c = Config::for_profile(Profile::HighAnonymity);
        assert!(!c.offline.enabled, "高匿名档必须关闭离线存储");
        assert!(!c.directory.enabled, "高匿名档必须关闭身份目录");
        assert!(c.idle_timeout_secs > 0, "高匿名档应有空闲超时");
    }

    #[test]
    fn beginner_enables_everything() {
        let c = Config::for_profile(Profile::Beginner);
        assert!(c.offline.enabled && c.group.enabled && c.directory.enabled);
    }

    #[test]
    fn directory_never_allows_arbitrary_lookup_by_default() {
        for p in [Profile::Beginner, Profile::Advanced, Profile::HighAnonymity] {
            assert!(!Config::for_profile(p).directory.allow_arbitrary_lookup);
        }
    }
}
