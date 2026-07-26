use super::deduplicate_and_merge_raw_notifier_events;
use notify_debouncer_full::{
    notify::{
        event::{ModifyKind, RenameMode},
        Event, EventKind,
    },
    DebouncedEvent,
};
use std::{path::PathBuf, time::Instant};

fn rename_any_event(path: PathBuf) -> DebouncedEvent {
    DebouncedEvent::new(
        Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::Any))).add_path(path),
        Instant::now(),
    )
}

#[cfg(unix)]
#[test]
fn rename_any_classifies_broken_symlink_destination_as_added() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::TempDir::new().unwrap();
    let destination = temp.path().join("renamed-broken-link");
    symlink("missing-target", &destination).unwrap();
    assert!(
        !destination.exists(),
        "Path::exists follows the missing target"
    );
    assert!(
        std::fs::symlink_metadata(&destination).is_ok(),
        "the lexical symlink inode must exist"
    );

    let update =
        deduplicate_and_merge_raw_notifier_events(&[rename_any_event(destination.clone())])
            .unwrap();

    assert_eq!(update.added, [destination].into_iter().collect());
    assert!(update.deleted.is_empty());
}

#[test]
fn rename_any_classifies_missing_source_as_deleted() {
    let temp = tempfile::TempDir::new().unwrap();
    let source = temp.path().join("removed-source");

    let update =
        deduplicate_and_merge_raw_notifier_events(&[rename_any_event(source.clone())]).unwrap();

    assert_eq!(update.deleted, [source].into_iter().collect());
    assert!(update.added.is_empty());
}
