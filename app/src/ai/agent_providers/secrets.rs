//! `AgentProviderSecrets`:把每个自定义 Provider 的 API key 保存到 OS 密钥库。
//!
//! 数据形态: `HashMap<provider_id, api_key>`,通过 `serde_json` 序列化后写入
//! `secure_storage` 的 `AgentProviderSecrets` 键。
//!
//! 设计参考 `crates/ai/src/api_keys.rs::ApiKeyManager`。

use std::collections::HashMap;

use warpui::{Entity, ModelContext, SingletonEntity};
use warpui_extras::secure_storage::{self, AppContextExt};

const SECURE_STORAGE_KEY: &str = "AgentProviderSecrets";

/// 当任意 Provider 的 API key 发生变化时发出。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentProviderSecretsEvent {
    KeysUpdated,
}

/// 单例:管理用户自定义 Provider 的 API key。
pub struct AgentProviderSecrets {
    keys: HashMap<String, String>,
}

impl AgentProviderSecrets {
    /// 启动时从 secure storage 读取所有 key。
    pub fn new(ctx: &mut ModelContext<Self>) -> Self {
        Self {
            keys: Self::load_from_storage(ctx),
        }
    }

    /// 读取指定 Provider 的 API key,若未配置则返回 `None`。
    pub fn get(&self, provider_id: &str) -> Option<&str> {
        self.keys.get(provider_id).map(String::as_str)
    }

    /// 设置/更新某个 Provider 的 API key。传入空字符串等价于删除。
    ///
    /// 返回 `Err` 表示 keychain 写入失败:此时内存状态会**回滚**到调用前,
    /// 使 [`get`](Self::get) 反映真实落库结果(写失败 ⇒ 不会显示为"已配置"),
    /// 调用方据此向用户上报失败。
    pub fn set(
        &mut self,
        provider_id: &str,
        api_key: String,
        ctx: &mut ModelContext<Self>,
    ) -> Result<(), secure_storage::Error> {
        let previous = if api_key.is_empty() {
            self.keys.remove(provider_id)
        } else {
            self.keys.insert(provider_id.to_owned(), api_key)
        };
        ctx.emit(AgentProviderSecretsEvent::KeysUpdated);
        if let Err(e) = self.persist(ctx) {
            self.restore(provider_id, previous, ctx);
            return Err(e);
        }
        Ok(())
    }

    /// 删除某个 Provider(连带其 secret)。写失败时回滚内存状态并返回 `Err`。
    pub fn remove(
        &mut self,
        provider_id: &str,
        ctx: &mut ModelContext<Self>,
    ) -> Result<(), secure_storage::Error> {
        let Some(previous) = self.keys.remove(provider_id) else {
            return Ok(());
        };
        ctx.emit(AgentProviderSecretsEvent::KeysUpdated);
        if let Err(e) = self.persist(ctx) {
            self.restore(provider_id, Some(previous), ctx);
            return Err(e);
        }
        Ok(())
    }

    /// 把某个 Provider 的内存 key 恢复到给定的先前值(用于 persist 失败回滚)。
    fn restore(
        &mut self,
        provider_id: &str,
        previous: Option<String>,
        ctx: &mut ModelContext<Self>,
    ) {
        match previous {
            Some(prev) => {
                self.keys.insert(provider_id.to_owned(), prev);
            }
            None => {
                self.keys.remove(provider_id);
            }
        }
        ctx.emit(AgentProviderSecretsEvent::KeysUpdated);
    }

    fn load_from_storage(ctx: &mut ModelContext<Self>) -> HashMap<String, String> {
        let raw = match ctx.secure_storage().read_value(SECURE_STORAGE_KEY) {
            Ok(json) => json,
            Err(secure_storage::Error::NotFound) => return HashMap::new(),
            Err(e) => {
                log::error!("Failed to read agent provider secrets: {e:#}");
                return HashMap::new();
            }
        };
        serde_json::from_str(&raw).unwrap_or_else(|e| {
            log::error!("Failed to deserialize agent provider secrets: {e:#}");
            HashMap::new()
        })
    }

    fn persist(&self, ctx: &mut ModelContext<Self>) -> Result<(), secure_storage::Error> {
        let json = serde_json::to_string(&self.keys)
            .map_err(|e| secure_storage::Error::Unknown(e.into()))?;
        ctx.secure_storage().write_value(SECURE_STORAGE_KEY, &json)
    }
}

impl Entity for AgentProviderSecrets {
    type Event = AgentProviderSecretsEvent;
}

impl SingletonEntity for AgentProviderSecrets {}
