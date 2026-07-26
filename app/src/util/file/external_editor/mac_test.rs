use super::is_ashide_bundle;

#[test]
fn is_ashide_bundle_recognises_ashide() {
    assert!(is_ashide_bundle("dev.ashide.Ashide"));
}

#[test]
fn is_ashide_bundle_rejects_other_apps() {
    assert!(!is_ashide_bundle("com.microsoft.VSCode"));
    assert!(!is_ashide_bundle("com.apple.TextEdit"));
    assert!(!is_ashide_bundle("dev.zed.Zed"));
    assert!(!is_ashide_bundle("dev.example.OtherPreview"));
    assert!(!is_ashide_bundle("invalid"));
    assert!(!is_ashide_bundle(""));
}
