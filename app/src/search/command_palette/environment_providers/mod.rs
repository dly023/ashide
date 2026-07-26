//! Command palette 数据源:Environment provider targets(Ashide 独有)。
//!
//! 用户在 Ctrl+Shift+P 中按 provider target 名称 / host 模糊匹配,选中后 emit
//! `OpenEnvironmentProviderTerminal`,由 Workspace 通过 Environment Runtime 打开 terminal
//! 并处理 provider secret；效果与从 Environment provider manager 中连接等价。

pub mod data_source;
pub mod search_item;

pub use data_source::EnvironmentProvidersDataSource;
pub use search_item::EnvironmentProviderSearchItem;
