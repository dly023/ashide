//! "Candidates" 区域的视图模型 —— 把 app-global SSH config catalog
//! 的 committed snapshot(及已导入别名集合、折叠状态)摊平成 UI 友好的 [`CandidateRow`]
//! 列表。
//!
//! 设计要点(对应 `specs/gh-110-ssh-config-import/{PRODUCT,TECH}.md`):
//!
//! - `rows()` 是**纯函数**:只依赖 view-model 的当前字段,不碰 IO / runtime,
//!   单元测试可以直接构造一个 `CandidatesViewModel` 并断言输出。这正是 TDD
//!   讨论里要求的点 —— PR 2 的渲染层 warpui 测试代价太高,把"哪些行该显示"
//!   的逻辑抽出来单测就够覆盖关键判断。
//! - source path、文件 IO 与 candidate generation 由 [`SshTargetCatalog`] 唯一持有；
//!   本模型只保存 panel presentation state。
//! - `on_tree_changed()` 由 panel 在订阅 `SshTreeChangedNotifier` 后调用 —— 把
//!   保存树里所有 server 的 `host` 字段收集成 `HashSet`,作为 "Added" 徽章的
//!   判定依据(PRODUCT.md decision E)。
//! - "已导入"的判定按 `host == alias` 做。导入逻辑在 panel 侧把 `server.host`
//!   设成候选别名(PRODUCT.md decision I),所以这里的比较语义与导入语义一致。
//!
//! 字段全部 `pub(crate)`,只让 `panel.rs` 看见;`CandidatesViewModel` 本身
//! 通过 `pub` 暴露给 `mod.rs` 的 re-export。

use std::collections::HashSet;

use warp_ssh_manager::LoadOutcome;
#[cfg(test)]
use warp_ssh_manager::{LoadResult, SshConfigCandidate};
use warpui::{Entity, ModelContext};

use crate::ssh_manager::SshTargetCatalog;

/// 共享 SSH config catalog 中一行候选服务器的 UI 状态视图。
pub struct CandidatesViewModel {
    /// 保存树里所有 server 的 `host` 字段集合。`rows()` 用它判断 `added`。
    added_aliases: HashSet<String>,
    /// 区段折叠状态(PRODUCT.md UX 表 "Many candidates")。默认展开。
    expanded: bool,
}

impl Default for CandidatesViewModel {
    fn default() -> Self {
        Self::new()
    }
}

impl CandidatesViewModel {
    /// 默认 presentation state。source snapshot 由 app-global catalog 提供。
    pub fn new() -> Self {
        Self {
            added_aliases: HashSet::new(),
            expanded: true,
        }
    }

    /// 测试用 presentation state 构造器。
    #[cfg(test)]
    pub fn with_state(added_aliases: HashSet<String>, expanded: bool) -> Self {
        Self {
            added_aliases,
            expanded,
        }
    }

    /// 树变更回调 —— 用传入的 server hosts 重建 `added_aliases`。
    ///
    /// 接收 `impl IntoIterator<Item = String>` 而不是 `&SshRepository` 让测试
    /// 不必塞一个真实的 SQLite 连接;调用方(panel)负责把 `list_nodes` +
    /// `get_server` 的 host 字段收集成迭代器再传入。
    pub fn on_tree_changed<I>(&mut self, hosts: I, ctx: &mut ModelContext<Self>)
    where
        I: IntoIterator<Item = String>,
    {
        self.added_aliases = hosts.into_iter().collect();
        ctx.notify();
    }

    /// 切换"区段折叠"状态。
    pub fn toggle_expanded(&mut self, ctx: &mut ModelContext<Self>) {
        self.expanded = !self.expanded;
        ctx.notify();
    }

    /// 是否展开(panel 渲染时决定是否显示 body 行)。
    pub fn is_expanded(&self) -> bool {
        self.expanded
    }

    /// 把当前状态摊平成行列表 —— 见模块文档的"纯函数"约定。
    ///
    /// 输出语义(对应 PRODUCT.md §5 UX 表):
    /// - `NotFound`:Header + 一行 `NotFound`。
    /// - `Error`:Header + 一行 `Error`(can_refresh=true 让用户改完 config 后重试)。
    /// - `Loaded(empty)`:Header + 一行 `Empty`。
    /// - `Loaded(non-empty)`:Header(count = N)+ N 行 `Candidate`,每行
    ///   `added` 由 `added_aliases.contains(alias)` 决定。
    pub fn rows(&self, catalog: &SshTargetCatalog) -> Vec<CandidateRow> {
        let path_display = catalog.config_path_display().unwrap_or_default();

        let mut out = Vec::new();
        let count = match catalog.outcome() {
            LoadOutcome::Loaded(v) => v.len(),
            LoadOutcome::NotFound | LoadOutcome::Error(_) => 0,
        };
        // Header 永远第一行 —— 即便区段折叠了,panel 仍要画 header(那是
        // toggle 入口)。`can_refresh = true` 总成立:任何状态都允许用户点
        // Refresh 重读。
        out.push(CandidateRow::Header {
            path_display: path_display.clone(),
            count,
            can_refresh: true,
        });

        // 区段折叠时只保留 header,body 不渲染。
        if !self.expanded {
            return out;
        }

        match catalog.outcome() {
            LoadOutcome::NotFound => {
                out.push(CandidateRow::NotFound { path_display });
            }
            LoadOutcome::Error(msg) => {
                out.push(CandidateRow::Error {
                    path_display,
                    message: msg.clone(),
                });
            }
            LoadOutcome::Loaded(v) if v.is_empty() => {
                out.push(CandidateRow::Empty { path_display });
            }
            LoadOutcome::Loaded(v) => {
                for c in v {
                    out.push(CandidateRow::Candidate {
                        alias: c.alias.clone(),
                        hostname: c.hostname.clone(),
                        user: c.user.clone(),
                        port: c.port,
                        identity_file: c.identity_file.as_ref().map(|p| p.display().to_string()),
                        added: self.added_aliases.contains(&c.alias),
                    });
                }
            }
        }

        out
    }
}

/// UI 友好的一行。Header 永远在最前面,后面要么是单条状态行(NotFound /
/// Empty / Error),要么是一串 Candidate。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CandidateRow {
    Header {
        path_display: String,
        count: usize,
        can_refresh: bool,
    },
    NotFound {
        path_display: String,
    },
    Empty {
        path_display: String,
    },
    Error {
        path_display: String,
        message: String,
    },
    Candidate {
        alias: String,
        hostname: Option<String>,
        user: Option<String>,
        port: Option<u16>,
        identity_file: Option<String>,
        added: bool,
    },
}

impl Entity for CandidatesViewModel {
    type Event = ();
}

#[cfg(test)]
#[path = "candidates_tests.rs"]
mod tests;

// 让测试代码不必关心 PathBuf 的具体磁盘路径 —— helper 用 `LoadResult` 拼一个
// 固定的展示串。测试模块里也会用到,所以放在外层以方便 #[cfg(test)] 复用。
#[cfg(test)]
pub(crate) fn fake_load_result_loaded(path: &str, cands: Vec<SshConfigCandidate>) -> LoadResult {
    LoadResult {
        path: Some(std::path::PathBuf::from(path)),
        outcome: LoadOutcome::Loaded(cands),
        has_unexpanded_includes: false,
    }
}

#[cfg(test)]
pub(crate) fn fake_load_result_not_found(path: &str) -> LoadResult {
    LoadResult {
        path: Some(std::path::PathBuf::from(path)),
        outcome: LoadOutcome::NotFound,
        has_unexpanded_includes: false,
    }
}

#[cfg(test)]
pub(crate) fn fake_load_result_error(path: &str, msg: &str) -> LoadResult {
    LoadResult {
        path: Some(std::path::PathBuf::from(path)),
        outcome: LoadOutcome::Error(msg.to_string()),
        has_unexpanded_includes: false,
    }
}
