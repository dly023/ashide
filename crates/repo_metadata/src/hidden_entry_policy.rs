use std::path::{Component, Path};

/// 隐藏项的唯一语义分类。优先级从内部元数据到普通可见项，确保内部目录
/// 即使同时是 dotfile 或 ignored，也绝不会被“显示隐藏文件”放行。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HiddenEntryKind {
    InternalMetadata,
    PlatformHidden,
    Ignored,
    Dotfile,
    Visible,
}

/// 消费者必须显式选择的隐藏项策略，禁止在调用方自行拼字符串过滤。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HiddenEntryPolicy {
    ProjectExplorer { show_user_hidden: bool },
    ContextChip,
}

impl HiddenEntryPolicy {
    pub fn classify(path: &Path, platform_hidden: bool, ignored: bool) -> HiddenEntryKind {
        if is_internal_metadata_path(path) {
            HiddenEntryKind::InternalMetadata
        } else if platform_hidden {
            HiddenEntryKind::PlatformHidden
        } else if ignored {
            HiddenEntryKind::Ignored
        } else if path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with('.'))
        {
            HiddenEntryKind::Dotfile
        } else {
            HiddenEntryKind::Visible
        }
    }

    pub fn allows(self, kind: HiddenEntryKind) -> bool {
        match (self, kind) {
            (_, HiddenEntryKind::InternalMetadata) => false,
            (_, HiddenEntryKind::Visible) => true,
            (Self::ProjectExplorer { show_user_hidden }, _) => show_user_hidden,
            (Self::ContextChip, _) => false,
        }
    }

    pub fn allows_path(self, path: &Path, platform_hidden: bool, ignored: bool) -> bool {
        self.allows(Self::classify(path, platform_hidden, ignored))
    }
}

pub fn is_internal_metadata_path(path: &Path) -> bool {
    path.components().any(|component| {
        matches!(
            component,
            Component::Normal(name)
                if name == ".git" || name == ".ashide-upload-staging"
        )
    })
}

pub fn platform_hidden(metadata: &std::fs::Metadata) -> bool {
    platform_hidden_impl(metadata)
}

#[cfg(target_os = "macos")]
fn platform_hidden_impl(metadata: &std::fs::Metadata) -> bool {
    use std::os::macos::fs::MetadataExt;

    metadata.st_flags() & libc::UF_HIDDEN != 0
}

#[cfg(target_os = "windows")]
fn platform_hidden_impl(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    metadata.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn platform_hidden_impl(_metadata: &std::fs::Metadata) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::{HiddenEntryKind, HiddenEntryPolicy};
    use std::path::Path;

    #[test]
    fn hidden_entry_policy_distinguishes_dot_platform_ignored_and_internal() {
        assert_eq!(
            HiddenEntryPolicy::classify(Path::new("/repo/.env"), false, false),
            HiddenEntryKind::Dotfile
        );
        assert_eq!(
            HiddenEntryPolicy::classify(Path::new("/repo/secret"), true, false),
            HiddenEntryKind::PlatformHidden
        );
        assert_eq!(
            HiddenEntryPolicy::classify(Path::new("/repo/target"), false, true),
            HiddenEntryKind::Ignored
        );
        assert_eq!(
            HiddenEntryPolicy::classify(Path::new("/repo/.git/HEAD"), true, true),
            HiddenEntryKind::InternalMetadata
        );
        assert_eq!(
            HiddenEntryPolicy::classify(Path::new("/repo/src"), false, false),
            HiddenEntryKind::Visible
        );
    }

    #[test]
    fn project_explorer_toggle_never_exposes_internal_metadata() {
        let show_hidden = HiddenEntryPolicy::ProjectExplorer {
            show_user_hidden: true,
        };
        assert!(show_hidden.allows(HiddenEntryKind::Dotfile));
        assert!(show_hidden.allows(HiddenEntryKind::PlatformHidden));
        assert!(show_hidden.allows(HiddenEntryKind::Ignored));
        assert!(!show_hidden.allows(HiddenEntryKind::InternalMetadata));
    }

    #[test]
    fn context_chip_declares_and_tests_its_hidden_categories() {
        let policy = HiddenEntryPolicy::ContextChip;
        assert!(policy.allows(HiddenEntryKind::Visible));
        assert!(!policy.allows(HiddenEntryKind::Dotfile));
        assert!(!policy.allows(HiddenEntryKind::PlatformHidden));
        assert!(!policy.allows(HiddenEntryKind::Ignored));
        assert!(!policy.allows(HiddenEntryKind::InternalMetadata));
    }
}
