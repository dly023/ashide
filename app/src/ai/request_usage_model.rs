//! Ashide(Phase 3c 子任务 A1):本地化为永远"无限额"stub。
//!
//! 历史职责:warp.dev 服务端 RPC 驱动的"每月 AI 请求配额"模型。
//! Ashide 走 BYOP(Bring Your Own Provider),用户自己付钱给 LLM 提供商,
//! 永远不应该被云端"剩余请求数 / 升级 CTA / 购买额外用量"等概念约束。
//!
//! 写入约束:
//! * 30+ UI 订阅点(`subscribe_to_model(&AIRequestUsageModel::handle(ctx), ...)`)
//!   保留,只是事件不再被任何路径触发 → 订阅回调成为永远静默的 no-op。
//! * 外溢使用 `RequestLimitInfo` / `RequestUsageInfo` /
//!   `RequestLimitRefreshDuration` / `AIRequestUsageModelEvent` 的文件
//!   (`ai_assistant/requests.rs`、`settings/ai.rs`、`settings/ai_tests.rs`、
//!   `settings_view/ai_page.rs`、`agent_view/agent_message_bar.rs`)
//!   不在本任务写入域内 → 必须在 stub 内继续保留这些类型定义与等价构造能力,
//!   只剥离 RPC / 缓存 / 计量等业务逻辑。

use crate::server_time::ServerTimestamp;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use warpui::{AppContext, Entity, ModelContext, SingletonEntity};

#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub enum RequestLimitRefreshDuration {
    Weekly,
    Monthly,
    EveryTwoWeeks,
}

/// 历史:服务端下发的"每月请求用量"快照。
/// Ashide:仅作为类型壳保留(`AISettings::update_quota_info` / `ai_assistant/requests.rs`
/// 等写入域外文件还会构造此结构)。`AIRequestUsageModel` 不再持有 / 缓存 / 更新它。
#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub struct RequestLimitInfo {
    pub limit: usize,
    pub num_requests_used_since_refresh: usize,
    pub next_refresh_time: ServerTimestamp,
    pub is_unlimited: bool,
    pub request_limit_refresh_duration: RequestLimitRefreshDuration,
    pub is_unlimited_voice: bool,
    #[serde(default)]
    pub voice_request_limit: usize,
    #[serde(default)]
    pub voice_requests_used_since_last_refresh: usize,
    #[serde(default)]
    pub max_files_per_repo: usize,
    #[serde(default)]
    pub embedding_generation_batch_size: usize,
}

fn default_voice_requests_limit() -> usize {
    10000
}

impl Default for RequestLimitInfo {
    /// Ashide:无云端配额,默认值视为"无限额"。
    fn default() -> Self {
        Self {
            limit: usize::MAX,
            num_requests_used_since_refresh: 0,
            next_refresh_time: ServerTimestamp::new(Utc::now() + chrono::Duration::days(365)),
            is_unlimited: true,
            request_limit_refresh_duration: RequestLimitRefreshDuration::Monthly,
            is_unlimited_voice: true,
            voice_request_limit: default_voice_requests_limit(),
            voice_requests_used_since_last_refresh: 0,
            max_files_per_repo: usize::MAX,
            embedding_generation_batch_size: 100,
        }
    }
}

#[cfg(test)]
impl RequestLimitInfo {
    pub fn new_for_test(limit: usize, num_requests_used_since_refresh: usize) -> Self {
        Self {
            limit,
            num_requests_used_since_refresh,
            ..Self::default()
        }
    }
}

/// 历史:服务端 `getRequestLimitInfo` 返回的聚合结构。
/// Ashide:仅作为类型壳保留(`ai_assistant/requests.rs` 仍会构造此类型)。
/// `AIRequestUsageModel` 不再消费它。
pub struct RequestUsageInfo {
    pub request_limit_info: RequestLimitInfo,
}

/// Ashide:Model 不再持有任何状态。
pub struct AIRequestUsageModel;

impl Entity for AIRequestUsageModel {
    type Event = AIRequestUsageModelEvent;
}

/// Ashide:保留 enum 定义以兼容订阅回调 `match` 模式;
/// `AIRequestUsageModel` 本地化后不再 emit 任何变体 → 所有订阅回调成为静默 no-op。
pub enum AIRequestUsageModelEvent {
    RequestUsageUpdated,
    RequestBonusRefunded {
        requests_refunded: i32,
        server_conversation_id: String,
        request_id: String,
    },
}

impl AIRequestUsageModel {
    pub fn new(_ctx: &mut ModelContext<Self>) -> Self {
        Self
    }

    #[cfg(test)]
    pub fn new_for_test(_ctx: &mut ModelContext<Self>) -> Self {
        Self
    }

    /// Ashide(本地化):永远返回 true,BYOP 本地运行不受云端限额约束。
    pub fn has_requests_remaining(&self) -> bool {
        true
    }

    /// Ashide(本地化):永远返回 true。
    /// AI 可用性仅取决于用户是否配置了 API key(由 `ApiKeyManager` 独立控制),
    /// 不该被 `request_limit_info` 等云端计量组件决定。
    pub fn has_any_ai_remaining(&self, _ctx: &AppContext) -> bool {
        true
    }

    /// Ashide(本地化):无云端计量,固定返回 0。
    pub fn requests_used(&self) -> usize {
        0
    }

    /// Ashide(本地化):无云端计量,固定返回 0.0。
    pub fn request_percentage_used(&self) -> f32 {
        0.0
    }

    /// Ashide(本地化):无云端 limit,固定返回 `usize::MAX`。
    pub fn request_limit(&self) -> usize {
        usize::MAX
    }

    /// Ashide(本地化):远期 placeholder 时间。
    pub fn next_refresh_time(&self) -> DateTime<Utc> {
        Utc::now() + chrono::Duration::days(365)
    }

    /// Ashide(本地化):永远无限制。
    pub fn is_unlimited(&self) -> bool {
        true
    }

    pub fn refresh_duration_to_string(&self) -> String {
        "monthly".to_string()
    }

    /// Ashide(本地化):语音输入不受云端用量限制,永远返回 true。
    pub fn can_request_voice(&self) -> bool {
        true
    }
}

impl SingletonEntity for AIRequestUsageModel {}
