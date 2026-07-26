use super::{PendingBufferMutation, PendingBufferMutationKind, ServerBufferTracker};
use crate::environment_runtime_transport::protocol::RequestId;
use crate::environment_runtime_transport::server_model::ConnectionId;
use warp_util::content_version::ContentVersion;
use warp_util::file::FileId;

fn connection_id() -> ConnectionId {
    uuid::Uuid::new_v4()
}

fn save_mutation(label: &str, conn_id: ConnectionId) -> PendingBufferMutation {
    PendingBufferMutation {
        request_id: RequestId::from(label.to_string()),
        conn_id,
        kind: PendingBufferMutationKind::SaveBuffer,
    }
}

fn resolve_mutation(label: &str, conn_id: ConnectionId) -> PendingBufferMutation {
    PendingBufferMutation {
        request_id: RequestId::from(label.to_string()),
        conn_id,
        kind: PendingBufferMutationKind::ResolveConflict {
            acknowledged_server_version: ContentVersion::new(),
            current_client_version: ContentVersion::new(),
            client_content: label.to_string(),
        },
    }
}

fn request_label(mutation: &PendingBufferMutation) -> String {
    mutation.request_id.clone().into()
}

#[test]
fn same_buffer_mutations_are_fifo_and_single_active() {
    let mut tracker = ServerBufferTracker::new();
    let file_id = FileId::new();
    let owner = connection_id();
    let other = connection_id();
    let third = connection_id();

    assert!(tracker.enqueue_mutation(file_id, save_mutation("first", owner)));
    assert!(!tracker.enqueue_mutation(file_id, resolve_mutation("second", other)));
    assert!(!tracker.enqueue_mutation(file_id, save_mutation("third", third)));

    let queue = &tracker.mutation_queues[&file_id];
    assert_eq!(request_label(queue.active.as_ref().unwrap()), "first");
    assert_eq!(queue.queued.len(), 2);
    assert_eq!(request_label(&queue.queued[0]), "second");
    assert_eq!(request_label(&queue.queued[1]), "third");
}

#[test]
fn completion_advances_exactly_one_buffer_mutation() {
    let mut tracker = ServerBufferTracker::new();
    let file_id = FileId::new();
    let owner = connection_id();
    tracker.enqueue_mutation(file_id, save_mutation("first", owner));
    tracker.enqueue_mutation(file_id, save_mutation("second", owner));
    tracker.enqueue_mutation(file_id, save_mutation("third", owner));

    let completed = tracker.complete_active_mutation(&file_id).unwrap();
    assert_eq!(request_label(&completed), "first");
    let queue = &tracker.mutation_queues[&file_id];
    assert_eq!(request_label(queue.active.as_ref().unwrap()), "second");
    assert_eq!(queue.queued.len(), 1);
    assert_eq!(request_label(&queue.queued[0]), "third");
}

#[test]
fn different_buffer_mutations_remain_independent() {
    let mut tracker = ServerBufferTracker::new();
    let first_file = FileId::new();
    let second_file = FileId::new();
    let owner = connection_id();

    assert!(tracker.enqueue_mutation(first_file, save_mutation("first-file", owner)));
    assert!(tracker.enqueue_mutation(second_file, save_mutation("second-file", owner)));
    assert_eq!(tracker.mutation_queues.len(), 2);
    assert_eq!(
        request_label(tracker.active_mutation(&first_file).unwrap()),
        "first-file"
    );
    assert_eq!(
        request_label(tracker.active_mutation(&second_file).unwrap()),
        "second-file"
    );
}

#[test]
fn disconnect_removes_queued_but_preserves_active_mutation() {
    let mut tracker = ServerBufferTracker::new();
    let file_id = FileId::new();
    let disconnecting = connection_id();
    let other = connection_id();
    let third = connection_id();
    tracker.enqueue_mutation(file_id, save_mutation("active", disconnecting));
    tracker.enqueue_mutation(file_id, save_mutation("drop-queued", disconnecting));
    tracker.enqueue_mutation(file_id, resolve_mutation("other", other));
    tracker.enqueue_mutation(file_id, save_mutation("third", third));

    tracker.remove_connection_pending_requests(disconnecting);

    let queue = &tracker.mutation_queues[&file_id];
    assert_eq!(request_label(queue.active.as_ref().unwrap()), "active");
    assert_eq!(queue.queued.len(), 2);
    assert_eq!(request_label(&queue.queued[0]), "other");
    assert_eq!(request_label(&queue.queued[1]), "third");
}

#[test]
fn failed_active_mutation_advances_next_intent() {
    let mut tracker = ServerBufferTracker::new();
    let file_id = FileId::new();
    let owner = connection_id();
    tracker.enqueue_mutation(file_id, resolve_mutation("failed", owner));
    tracker.enqueue_mutation(file_id, save_mutation("retry", owner));

    let failed = tracker.complete_active_mutation(&file_id).unwrap();
    assert_eq!(request_label(&failed), "failed");
    assert_eq!(
        request_label(tracker.active_mutation(&file_id).unwrap()),
        "retry"
    );
    assert!(tracker.mutation_queues[&file_id].queued.is_empty());
}

#[test]
fn second_connection_cannot_open_or_mutate_owned_buffer() {
    let mut tracker = ServerBufferTracker::new();
    let file_id = FileId::new();
    let owner = connection_id();
    let foreign = connection_id();

    assert!(tracker.add_connection(file_id, owner));
    assert!(!tracker.add_connection(file_id, foreign));

    assert_eq!(tracker.connection_for_buffer(&file_id), Some(owner));
    assert!(tracker.is_writer(file_id, owner));
    assert!(!tracker.is_writer(file_id, foreign));
}

#[test]
fn foreign_buffer_mutations_use_shared_writer_guard() {
    let mut tracker = ServerBufferTracker::new();
    let file_id = FileId::new();
    let owner = connection_id();
    let foreign = connection_id();
    let path = "/tmp/owned.txt";
    tracker.open_buffers.insert(path.to_owned(), file_id);
    assert!(tracker.add_connection(file_id, owner));

    assert_eq!(
        tracker.require_writer(path, foreign),
        Err(super::BufferWriterAccessError::NotOwner)
    );
    assert_eq!(
        tracker.require_writer("/tmp/not-open.txt", owner),
        Err(super::BufferWriterAccessError::NotOpen)
    );
    assert_eq!(tracker.require_writer(path, owner), Ok(file_id));
}

#[test]
fn foreign_close_does_not_release_owner() {
    let mut tracker = ServerBufferTracker::new();
    let file_id = FileId::new();
    let owner = connection_id();
    let foreign = connection_id();
    let path = "/tmp/owned.txt";
    tracker.open_buffers.insert(path.to_owned(), file_id);
    assert!(tracker.add_connection(file_id, owner));

    assert_eq!(tracker.release_writer(path, foreign), None);
    assert_eq!(tracker.connection_for_buffer(&file_id), Some(owner));
    assert_eq!(tracker.release_writer(path, owner), Some(file_id));
    assert_eq!(tracker.connection_for_buffer(&file_id), None);
}

#[test]
fn owner_disconnect_releases_buffer_writer_owner() {
    let mut tracker = ServerBufferTracker::new();
    let first_file = FileId::new();
    let second_file = FileId::new();
    let owner = connection_id();
    let peer = connection_id();
    assert!(tracker.add_connection(first_file, owner));
    assert!(tracker.add_connection(second_file, peer));

    let released = tracker.release_connection_ownership(owner);
    assert_eq!(released, vec![first_file]);
    assert_eq!(tracker.connection_for_buffer(&first_file), None);
    assert_eq!(tracker.connection_for_buffer(&second_file), Some(peer));
}

#[test]
fn failed_open_releases_path_writer_and_pending_ownership_for_retry() {
    let mut tracker = ServerBufferTracker::new();
    let failed_file = FileId::new();
    let retry_file = FileId::new();
    let owner = connection_id();
    let retrying_connection = connection_id();
    let path = "/tmp/retry.txt";

    tracker.open_buffers.insert(path.to_owned(), failed_file);
    assert!(tracker.add_connection(failed_file, owner));
    tracker.insert_pending_open(failed_file, RequestId::from("open-1".to_owned()), owner);
    tracker.insert_pending_open(failed_file, RequestId::from("open-2".to_owned()), owner);
    tracker.enqueue_mutation(failed_file, save_mutation("save", owner));

    let (pending_opens, pending_mutations) = tracker.fail_open_buffer(&failed_file);

    assert_eq!(pending_opens.len(), 2);
    assert_eq!(pending_mutations.len(), 1);
    assert_eq!(request_label(&pending_mutations[0]), "save");
    assert_eq!(tracker.file_id_for_path(path), None);
    assert_eq!(tracker.connection_for_buffer(&failed_file), None);
    assert!(tracker.active_mutation(&failed_file).is_none());

    tracker.open_buffers.insert(path.to_owned(), retry_file);
    assert!(tracker.add_connection(retry_file, owner));
    assert_eq!(tracker.file_id_for_path(path), Some(retry_file));
    assert_eq!(tracker.release_writer(path, owner), Some(retry_file));
    tracker.open_buffers.remove_by_right(&retry_file);

    let peer_retry_file = FileId::new();
    tracker
        .open_buffers
        .insert(path.to_owned(), peer_retry_file);
    assert!(tracker.add_connection(peer_retry_file, retrying_connection));
    assert_eq!(tracker.file_id_for_path(path), Some(peer_retry_file));
    assert_eq!(
        tracker.connection_for_buffer(&peer_retry_file),
        Some(retrying_connection)
    );
}
