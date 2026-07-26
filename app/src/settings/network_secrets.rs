//! `ProxyCredentials`:把代理 Basic Auth 的密码保存到 OS 密钥库(见 Issue #72)。
//!
//! 仅存密码;用户名、URL 等非敏感字段仍在 `NetworkSettings` 的 settings.toml 里。
//! 设计形态与 `crate::ai::agent_providers::AgentProviderSecrets` 一致:基于
//! `warpui_extras::secure_storage`(macOS Keychain / Windows DPAPI / Linux Keyring)。
//!
//! 注意:代理只有一个全局 password,因此存储里就一个 key、value 是原始 password
//! 字符串(不再走 JSON map)。

use warpui::{Entity, ModelContext, SingletonEntity};
use warpui_extras::secure_storage::{self, AppContextExt};

const SECURE_STORAGE_KEY: &str = "ProxyPassword";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProxyCredentialsEvent {
    /// 密码值变化(可能为空)。
    PasswordChanged,
}

/// 单例:管理全局 HTTP 代理的 Basic Auth 密码。
pub struct ProxyCredentials {
    password: String,
}

impl ProxyCredentials {
    pub fn new(ctx: &mut ModelContext<Self>) -> Self {
        Self {
            password: Self::load_from_storage(ctx),
        }
    }

    /// 读取当前密码;无值时返回空串。
    pub fn password(&self) -> &str {
        &self.password
    }

    /// 设置 / 更新密码。传入空串等价于删除。
    ///
    /// 返回 `Err` 表示 keychain 写入失败:此时内存密码会**回滚**到调用前的值
    /// (并且不发出 `PasswordChanged`),使 [`password`](Self::password) 反映真实
    /// 落库结果,调用方据此向用户上报失败。
    pub fn set_password(
        &mut self,
        password: String,
        ctx: &mut ModelContext<Self>,
    ) -> Result<(), secure_storage::Error> {
        if self.password == password {
            return Ok(());
        }
        let previous = std::mem::replace(&mut self.password, password);
        if let Err(e) = self.persist(ctx) {
            self.password = previous;
            return Err(e);
        }
        ctx.emit(ProxyCredentialsEvent::PasswordChanged);
        Ok(())
    }

    fn load_from_storage(ctx: &mut ModelContext<Self>) -> String {
        match ctx.secure_storage().read_value(SECURE_STORAGE_KEY) {
            Ok(value) => value,
            Err(secure_storage::Error::NotFound) => String::new(),
            Err(e) => {
                log::error!("Failed to read proxy password: {e:#}");
                String::new()
            }
        }
    }

    fn persist(&self, ctx: &mut ModelContext<Self>) -> Result<(), secure_storage::Error> {
        if self.password.is_empty() {
            // 空字符串语义为"无密码";删除时 NotFound 视为成功(本就没有)。
            return match ctx.secure_storage().remove_value(SECURE_STORAGE_KEY) {
                Ok(()) | Err(secure_storage::Error::NotFound) => Ok(()),
                Err(e) => Err(e),
            };
        }
        ctx.secure_storage()
            .write_value(SECURE_STORAGE_KEY, &self.password)
    }
}

impl Entity for ProxyCredentials {
    type Event = ProxyCredentialsEvent;
}

impl SingletonEntity for ProxyCredentials {}
