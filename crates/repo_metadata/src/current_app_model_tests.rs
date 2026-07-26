//! Tests for the CurrentAppRepoMetadataModel.

#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests {
    use crate::current_app_model::{
        CurrentAppRepoMetadataModel, GetContentsArgs, IndexedRepoState, RepoUpdate,
        RepositoryMetadataEvent, WatchDepth, WatchPathOwner,
    };
    use crate::entry::{DirectoryEntry, Entry, FileMetadata, IgnoredPathStrategy};
    use crate::file_tree_store::{FileTreeEntry, FileTreeEntryState, FileTreeState};
    use crate::repositories::DetectedRepositories;
    use crate::watcher::DirectoryWatcher;
    use futures::channel::oneshot;
    use futures::executor::block_on;
    use ignore::gitignore::Gitignore;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::rc::Rc;
    use std::time::Duration;
    use virtual_fs::{Stub, VirtualFS};
    use warp_util::standardized_path::StandardizedPath;
    use warpui::r#async::FutureExt as _;
    use warpui::App;

    impl CurrentAppRepoMetadataModel {
        fn new_for_test() -> Self {
            Self {
                repositories: HashMap::new(),
                lazy_loaded_paths: Default::default(),
                #[cfg(feature = "local_fs")]
                watcher: Default::default(),
                #[cfg(feature = "local_fs")]
                symlink_watch_mounts: Default::default(),
                #[cfg(feature = "local_fs")]
                watch_path_owners: Default::default(),
                #[cfg(feature = "local_fs")]
                pending_repo_updates: Default::default(),
                #[cfg(feature = "local_fs")]
                repo_update_in_flight: Default::default(),
                #[cfg(feature = "local_fs")]
                next_repo_update_token: 1,
                emit_incremental_updates: false,
            }
        }
    }

    #[test]
    fn test_get_repo_contents() {
        VirtualFS::test("repo_contents_test", |dirs, mut vfs| {
            let test_repo = dirs.tests().join("test_repo");

            // Create a test repository structure using VirtualFS with .git directory
            vfs.mkdir("test_repo/.git/objects")
                .mkdir("test_repo/subdir")
                .with_files(vec![
                    Stub::FileWithContent("test_repo/.git/HEAD", "ref: refs/heads/main"),
                    Stub::FileWithContent(
                        "test_repo/.git/config",
                        "[core]\n\trepositoryformatversion = 0",
                    ),
                    Stub::FileWithContent("test_repo/file1.txt", "content1"),
                    Stub::FileWithContent("test_repo/subdir/file2.rs", "content2"),
                    Stub::FileWithContent("test_repo/subdir/file3.py", "content3"),
                    Stub::FileWithContent("test_repo/file4.md", "content4"),
                    Stub::FileWithContent("test_repo/.gitignore", ""),
                ]);

            // Create a mock file tree structure
            let file1 = Entry::File(FileMetadata::new(test_repo.join("file1.txt"), false));
            let file2 = Entry::File(FileMetadata::new(test_repo.join("subdir/file2.rs"), false));
            let file3 = Entry::File(FileMetadata::new(test_repo.join("subdir/file3.py"), false));
            let file4 = Entry::File(FileMetadata::new(test_repo.join("file4.md"), false));

            let subdir = Entry::Directory(DirectoryEntry {
                path: StandardizedPath::try_from_local(&test_repo.join("subdir")).unwrap(),
                children: vec![file2, file3],
                ignored: false,
                loaded: true,
            });

            let root = Entry::Directory(DirectoryEntry {
                path: StandardizedPath::try_from_local(&test_repo).unwrap(),
                children: vec![file1, subdir, file4],
                ignored: false,
                loaded: true,
            });

            let (gitignore, _) = Gitignore::new(test_repo.join(".gitignore"));

            App::test((), |mut app| async move {
                // Create RepoWatcher and get Repository handle through it
                let repo_watcher = app.add_singleton_model(DirectoryWatcher::new);
                let repo_handle = repo_watcher.update(&mut app, |repo_watcher, ctx| {
                    repo_watcher
                        .add_directory(
                            StandardizedPath::from_local_canonicalized(&test_repo).unwrap(),
                            ctx,
                        )
                        .unwrap()
                });
                let state = FileTreeState::new(root, vec![gitignore], Some(repo_handle));

                let model_handle = app.add_model(|_| CurrentAppRepoMetadataModel::new_for_test());

                model_handle.update(&mut app, |model, _ctx| {
                    // Use the CanonicalizedPath as the key
                    let canonical_key =
                        StandardizedPath::from_local_canonicalized(&test_repo).unwrap();
                    model
                        .repositories
                        .insert(canonical_key, IndexedRepoState::Indexed(state));
                });

                // Test getting all files
                model_handle.read(&app, |model, _ctx| {
                    let args = GetContentsArgs {
                        include_folders: false,
                        include_ignored: false,
                        filter: None,
                    };
                    let files = model
                        .get_repo_contents(
                            &StandardizedPath::from_local_canonicalized(&test_repo).unwrap(),
                            args,
                        )
                        .unwrap();

                    // Should have 4 files total (file1.txt, file2.rs, file3.py, file4.md)
                    assert_eq!(files.len(), 4);

                    // Test with non-existent repository
                    let non_existent = StandardizedPath::try_new("/non_existent_repo").unwrap();
                    let args = GetContentsArgs {
                        include_folders: false,
                        include_ignored: false,
                        filter: None,
                    };
                    let non_existent_result = model.get_repo_contents(&non_existent, args);
                    assert!(non_existent_result.is_none());
                });
            });
        });
    }

    #[cfg(feature = "local_fs")]
    #[test]
    fn test_lazy_loaded_path_registrations_are_refcounted() {
        VirtualFS::test("lazy_loaded_path_refcount", |dirs, mut vfs| {
            vfs.mkdir("shared_dir")
                .with_files(vec![Stub::FileWithContent(
                    "shared_dir/file.txt",
                    "content",
                )]);

            let shared_dir = dirs.tests().join("shared_dir");

            App::test((), |mut app| async move {
                let model_handle = app.add_model(|_| CurrentAppRepoMetadataModel::new_for_test());

                let shared_dir_for_index =
                    StandardizedPath::from_local_canonicalized(&shared_dir).unwrap();
                model_handle.update(&mut app, |model, ctx| {
                    model
                        .index_lazy_loaded_path(&shared_dir_for_index, ctx)
                        .unwrap();
                    model
                        .index_lazy_loaded_path(&shared_dir_for_index, ctx)
                        .unwrap();
                });

                model_handle.read(&app, |model, _ctx| {
                    assert!(model.is_lazy_loaded_path(
                        &StandardizedPath::from_local_canonicalized(&shared_dir).unwrap()
                    ));
                    assert!(model.has_repository(
                        &StandardizedPath::from_local_canonicalized(&shared_dir).unwrap()
                    ));
                });

                let shared_dir_std =
                    StandardizedPath::from_local_canonicalized(&shared_dir).unwrap();

                model_handle.update(&mut app, |model, ctx| {
                    model.remove_lazy_loaded_path(&shared_dir_std, ctx);
                });

                model_handle.read(&app, |model, _ctx| {
                    assert!(model.is_lazy_loaded_path(&shared_dir_std));
                    assert!(model.has_repository(&shared_dir_std));
                });

                model_handle.update(&mut app, |model, ctx| {
                    model.remove_lazy_loaded_path(&shared_dir_std, ctx);
                });

                model_handle.read(&app, |model, _ctx| {
                    assert!(!model.is_lazy_loaded_path(
                        &StandardizedPath::from_local_canonicalized(&shared_dir).unwrap()
                    ));
                    assert!(!model.has_repository(
                        &StandardizedPath::from_local_canonicalized(&shared_dir).unwrap()
                    ));
                });
            });
        });
    }

    #[cfg(feature = "local_fs")]
    #[test]
    fn test_index_directory_upgrades_lazy_loaded_path_to_repo() {
        VirtualFS::test("lazy_loaded_path_upgrade", |dirs, mut vfs| {
            vfs.mkdir("repo/.git/objects")
                .mkdir("repo/src/nested")
                .with_files(vec![
                    Stub::FileWithContent("repo/.git/HEAD", "ref: refs/heads/main"),
                    Stub::FileWithContent(
                        "repo/.git/config",
                        "[core]\n\trepositoryformatversion = 0",
                    ),
                    Stub::FileWithContent("repo/src/nested/main.rs", "fn main() {}\n"),
                ]);

            let repo_root = dirs.tests().join("repo");
            let src_dir = repo_root.join("src");
            let source_file = repo_root.join("src/nested/main.rs");

            App::test((), |mut app| async move {
                let directory_watcher = app.add_singleton_model(DirectoryWatcher::new);
                let repository_handle = directory_watcher.update(&mut app, |watcher, ctx| {
                    watcher
                        .add_directory(
                            StandardizedPath::from_local_canonicalized(&repo_root).unwrap(),
                            ctx,
                        )
                        .unwrap()
                });
                let model_handle = app.add_model(|_| CurrentAppRepoMetadataModel::new_for_test());

                let repo_root_for_index =
                    StandardizedPath::from_local_canonicalized(&repo_root).unwrap();
                model_handle.update(&mut app, |model, ctx| {
                    model
                        .index_lazy_loaded_path(&repo_root_for_index, ctx)
                        .unwrap();
                });

                model_handle.read(&app, |model, _ctx| {
                    assert!(model.is_lazy_loaded_path(
                        &StandardizedPath::from_local_canonicalized(&repo_root).unwrap()
                    ));
                    let Some(IndexedRepoState::Indexed(state)) = model.repository_state(
                        &StandardizedPath::from_local_canonicalized(&repo_root).unwrap(),
                    ) else {
                        panic!("expected indexed lazy-loaded path");
                    };
                    assert!(state
                        .entry
                        .contains(&StandardizedPath::try_from_local(&src_dir).unwrap()));
                    assert!(!state
                        .entry
                        .contains(&StandardizedPath::try_from_local(&source_file).unwrap()));
                });

                let (tx, rx) = oneshot::channel();
                let repo_root_for_event = repo_root.clone();
                let upgrade_completed = Rc::new(RefCell::new(Some(tx)));
                let upgrade_completed_for_event = upgrade_completed.clone();
                app.update(|ctx| {
                    ctx.subscribe_to_model(&model_handle, move |_, event, _ctx| {
                        if matches!(
                            event,
                            RepositoryMetadataEvent::RepositoryUpdated { path }
                                if path.to_local_path().as_ref() == Some(&repo_root_for_event)
                        ) {
                            if let Some(tx) = upgrade_completed_for_event.borrow_mut().take() {
                                let _ = tx.send(());
                            }
                        }
                    });
                });

                model_handle.update(&mut app, |model, ctx| {
                    model.index_directory(repository_handle, ctx).unwrap();
                });
                rx.with_timeout(Duration::from_secs(5))
                    .await
                    .expect("timed out waiting for repo upgrade")
                    .expect("repo upgrade completion sender dropped");

                model_handle.read(&app, |model, _ctx| {
                    assert!(!model.is_lazy_loaded_path(
                        &StandardizedPath::from_local_canonicalized(&repo_root).unwrap()
                    ));
                    let Some(IndexedRepoState::Indexed(state)) = model.repository_state(
                        &StandardizedPath::from_local_canonicalized(&repo_root).unwrap(),
                    ) else {
                        panic!("expected indexed repo after upgrade");
                    };
                    assert!(state
                        .entry
                        .contains(&StandardizedPath::try_from_local(&source_file).unwrap()));
                });
            });
        });
    }

    #[test]
    fn test_get_repo_contents_include_ignored() {
        VirtualFS::test("repo_contents_include_ignored_test", |dirs, mut vfs| {
            let test_repo = dirs.tests().join("test_repo");

            // Create a test repository structure with both ignored and non-ignored files
            vfs.mkdir("test_repo/.git/objects")
                .mkdir("test_repo/src")
                .mkdir("test_repo/target/debug")
                .mkdir("test_repo/node_modules")
                .with_files(vec![
                    Stub::FileWithContent("test_repo/.git/HEAD", "ref: refs/heads/main"),
                    Stub::FileWithContent(
                        "test_repo/.git/config",
                        "[core]\n\trepositoryformatversion = 0",
                    ),
                    Stub::FileWithContent("test_repo/src/main.rs", "fn main() {}"),
                    Stub::FileWithContent("test_repo/README.md", "# Project"),
                    Stub::FileWithContent("test_repo/target/debug/binary", "binary"),
                    Stub::FileWithContent("test_repo/node_modules/package.json", "{}"),
                    Stub::FileWithContent("test_repo/debug.log", "log"),
                    Stub::FileWithContent("test_repo/.gitignore", "*.log\n/target/\nnode_modules/"),
                ]);

            // Create mock file tree with ignored and non-ignored entries
            let main_rs = Entry::File(FileMetadata::new(test_repo.join("src/main.rs"), false));
            let readme = Entry::File(FileMetadata::new(test_repo.join("README.md"), false));
            let debug_log = Entry::File(FileMetadata::new(test_repo.join("debug.log"), true));
            let binary = Entry::File(FileMetadata::new(
                test_repo.join("target/debug/binary"),
                true,
            ));
            let package_json = Entry::File(FileMetadata::new(
                test_repo.join("node_modules/package.json"),
                true,
            ));

            let src_dir = Entry::Directory(DirectoryEntry {
                path: StandardizedPath::try_from_local(&test_repo.join("src")).unwrap(),
                children: vec![main_rs],
                ignored: false,
                loaded: true,
            });

            let debug_dir = Entry::Directory(DirectoryEntry {
                path: StandardizedPath::try_from_local(&test_repo.join("target/debug")).unwrap(),
                children: vec![binary],
                ignored: true,
                loaded: true,
            });

            let target_dir = Entry::Directory(DirectoryEntry {
                path: StandardizedPath::try_from_local(&test_repo.join("target")).unwrap(),
                children: vec![debug_dir],
                ignored: true,
                loaded: true,
            });

            let node_modules_dir = Entry::Directory(DirectoryEntry {
                path: StandardizedPath::try_from_local(&test_repo.join("node_modules")).unwrap(),
                children: vec![package_json],
                ignored: true,
                loaded: true,
            });

            let root = Entry::Directory(DirectoryEntry {
                path: StandardizedPath::try_from_local(&test_repo).unwrap(),
                children: vec![src_dir, readme, debug_log, target_dir, node_modules_dir],
                ignored: false,
                loaded: true,
            });

            let (gitignore, _) = Gitignore::new(test_repo.join(".gitignore"));

            App::test((), |mut app| async move {
                let repo_watcher = app.add_singleton_model(DirectoryWatcher::new);
                let repo_handle = repo_watcher.update(&mut app, |repo_watcher, ctx| {
                    repo_watcher
                        .add_directory(
                            StandardizedPath::from_local_canonicalized(&test_repo).unwrap(),
                            ctx,
                        )
                        .unwrap()
                });
                let state = FileTreeState::new(root, vec![gitignore], Some(repo_handle));

                let model_handle = app.add_model(|_| CurrentAppRepoMetadataModel::new_for_test());

                model_handle.update(&mut app, |model, _ctx| {
                    let canonical_key =
                        StandardizedPath::from_local_canonicalized(&test_repo).unwrap();
                    model
                        .repositories
                        .insert(canonical_key, IndexedRepoState::Indexed(state));
                });

                // Test with include_ignored = false (should exclude ignored files and directories)
                model_handle.read(&app, |model, _ctx| {
                    let args = GetContentsArgs {
                        include_folders: true,
                        include_ignored: false,
                        filter: None,
                    };
                    let contents = model
                        .get_repo_contents(
                            &StandardizedPath::from_local_canonicalized(&test_repo).unwrap(),
                            args,
                        )
                        .unwrap();

                    let paths: Vec<PathBuf> = contents
                        .iter()
                        .map(|c| match c {
                            crate::RepoContent::File(f) => f.path.to_local_path_lossy(),
                            crate::RepoContent::Directory(d) => d.path.to_local_path_lossy(),
                            crate::RepoContent::Symlink(s) => s.path.to_local_path_lossy(),
                        })
                        .collect();

                    // Should include non-ignored files and directories
                    assert!(paths.contains(&test_repo.join("src")));
                    assert!(paths.contains(&test_repo.join("src/main.rs")));
                    assert!(paths.contains(&test_repo.join("README.md")));

                    // Should NOT include ignored directories or files
                    assert!(!paths.contains(&test_repo.join("target")));
                    assert!(!paths.contains(&test_repo.join("node_modules")));
                    assert!(!paths.contains(&test_repo.join("debug.log")));
                });

                // Test with include_ignored = true (should include everything)
                model_handle.read(&app, |model, _ctx| {
                    let args = GetContentsArgs {
                        include_folders: true,
                        include_ignored: true,
                        filter: None,
                    };
                    let contents = model
                        .get_repo_contents(
                            &StandardizedPath::from_local_canonicalized(&test_repo).unwrap(),
                            args,
                        )
                        .unwrap();

                    let paths: Vec<PathBuf> = contents
                        .iter()
                        .map(|c| match c {
                            crate::RepoContent::File(f) => f.path.to_local_path_lossy(),
                            crate::RepoContent::Directory(d) => d.path.to_local_path_lossy(),
                            crate::RepoContent::Symlink(s) => s.path.to_local_path_lossy(),
                        })
                        .collect();

                    // Should include everything
                    assert!(paths.contains(&test_repo.join("src")));
                    assert!(paths.contains(&test_repo.join("target")));
                    assert!(paths.contains(&test_repo.join("target/debug")));
                    assert!(paths.contains(&test_repo.join("node_modules")));
                    assert!(paths.contains(&test_repo.join("src/main.rs")));
                    assert!(paths.contains(&test_repo.join("README.md")));
                    assert!(paths.contains(&test_repo.join("debug.log")));
                    assert!(paths.contains(&test_repo.join("target/debug/binary")));
                    assert!(paths.contains(&test_repo.join("node_modules/package.json")));
                });
            });
        });
    }

    #[test]
    fn test_should_include_path_respects_gitignore() {
        VirtualFS::test("gitignore_test", |dirs, mut fs| {
            let repo_path = dirs.tests();

            // Create directory structure and files using VirtualFS
            fs.mkdir("src")
                .mkdir("target/debug")
                .mkdir("node_modules/package")
                .mkdir("docs")
                .with_files(vec![
                    Stub::FileWithContent("debug.log", "log"),
                    Stub::FileWithContent("target/debug/main", "binary"),
                    Stub::FileWithContent("node_modules/package/index.js", "js"),
                    Stub::FileWithContent(".env", "env"),
                    Stub::FileWithContent("src/main.rs", "rust"),
                    Stub::FileWithContent("README.md", "readme"),
                    Stub::FileWithContent("package.json", "json"),
                    Stub::FileWithContent("docs/guide.md", "guide"),
                    Stub::FileWithContent(".gitignore", "*.log\n/target/\nnode_modules/\n.env"),
                ]);

            let gitignore_path = repo_path.join(".gitignore");

            // Create the gitignore object
            let (gitignore, _) = Gitignore::new(&gitignore_path);
            let gitignores = vec![gitignore];

            // Test files that should be excluded
            let excluded_paths = vec![
                repo_path.join("debug.log"),
                repo_path.join("target").join("debug").join("main"),
                repo_path
                    .join("node_modules")
                    .join("package")
                    .join("index.js"),
                repo_path.join(".env"),
            ];

            for path in excluded_paths {
                assert!(
                    CurrentAppRepoMetadataModel::path_is_ignored(&path, &gitignores),
                    "Path should be excluded by gitignore: {path:?}"
                );
            }

            // Test files that should be included
            let included_paths = vec![
                repo_path.join("src").join("main.rs"),
                repo_path.join("README.md"),
                repo_path.join("package.json"),
                repo_path.join("docs").join("guide.md"),
            ];

            for path in included_paths {
                assert!(
                    !CurrentAppRepoMetadataModel::path_is_ignored(&path, &gitignores),
                    "Path should be included: {path:?}"
                );
            }
        });
    }

    #[test]
    fn test_update_file_tree_entry_respects_gitignore() {
        VirtualFS::test("tree_update_test", |dirs, mut fs| {
            let repo_path = dirs.tests();

            // Create initial directory structure and files
            fs.mkdir("src")
                .with_files(vec![
                    Stub::FileWithContent("src/main.rs", "fn main() {}"),
                    Stub::FileWithContent(".gitignore", "*.log\n/target/"),
                    Stub::FileWithContent("debug.log", "log content"),
                    Stub::FileWithContent("README.md", "# Project"),
                ])
                .mkdir("target");

            let gitignore_path = repo_path.join(".gitignore");
            let (gitignore, _) = Gitignore::new(&gitignore_path);
            let gitignores = vec![gitignore];

            // Create an initial file tree
            let root_entry = Entry::Directory(DirectoryEntry {
                path: StandardizedPath::try_from_local(repo_path).unwrap(),
                children: vec![Entry::Directory(DirectoryEntry {
                    path: StandardizedPath::try_from_local(&repo_path.join("src")).unwrap(),
                    children: vec![Entry::File(FileMetadata::new(
                        repo_path.join("src").join("main.rs"),
                        false,
                    ))],
                    ignored: false,
                    loaded: true,
                })],
                ignored: false,
                loaded: true,
            });
            let mut root = FileTreeEntry::from(root_entry);

            // Create files to test adding - some should be ignored
            let log_file = repo_path.join("debug.log");
            let target_dir = repo_path.join("target");
            let readme_file = repo_path.join("README.md");

            // Create update with both ignored and allowed files
            let update = RepoUpdate {
                added: vec![log_file.clone(), readme_file.clone(), target_dir.clone()],
                deleted: vec![],
                moved: HashMap::new(),
                refreshed: vec![],
            };

            // Compute mutations on the "background thread" then apply on the "main thread".
            let mutations = block_on(CurrentAppRepoMetadataModel::compute_file_tree_mutations(
                &update,
                &gitignores,
            ));
            CurrentAppRepoMetadataModel::apply_file_tree_mutations(
                &mut root, mutations, false, false,
            );

            // Verify that only the README.md was added (log file and target dir should be ignored)
            let mut all_paths = Vec::new();
            collect_all_paths(&root, &mut all_paths);

            // Should contain all files
            let readme_std = StandardizedPath::try_from_local(&readme_file).unwrap();
            let log_std = StandardizedPath::try_from_local(&log_file).unwrap();
            let target_std = StandardizedPath::try_from_local(&target_dir).unwrap();
            assert!(all_paths.contains(&readme_std));
            assert!(all_paths.contains(&log_std));
            assert!(all_paths.contains(&target_std));

            // Make sure that the ignored files and folders are marked as ignored.
            assert!(root
                .get(&StandardizedPath::try_from_local(&log_file).unwrap())
                .unwrap()
                .ignored());
            assert!(root
                .get(&StandardizedPath::try_from_local(&target_dir).unwrap())
                .unwrap()
                .ignored());

            // Make sure that the ignored folder is not eagerly loaded.
            assert!(!root
                .get(&StandardizedPath::try_from_local(&target_dir).unwrap())
                .unwrap()
                .loaded());
        });
    }

    #[test]
    fn test_gitignore_patterns_comprehensive() {
        VirtualFS::test("comprehensive_test", |dirs, mut fs| {
            let repo_path = dirs.tests();

            // Create directory structure and files using VirtualFS
            fs.mkdir("target/debug")
                .mkdir("dist")
                .mkdir("build")
                .mkdir("logs")
                .mkdir("node_modules/react")
                .mkdir("vendor")
                .mkdir(".vscode")
                .mkdir(".idea")
                .mkdir("src")
                .mkdir("docs")
                .mkdir("tests")
                .mkdir(".github/workflows");

            // Create a comprehensive .gitignore
            let gitignore_content = r#"
# Build outputs
/target/
/dist/
build/

# Logs
*.log
logs/

# Dependencies
node_modules/
/vendor/

# IDE files
.vscode/
.idea/
*.swp

# Environment
.env
.env.local

# OS files
.DS_Store
Thumbs.db
"#;

            // Create all files
            fs.with_files(vec![
                Stub::FileWithContent("target/debug/main", "binary"),
                Stub::FileWithContent("dist/bundle.js", "js"),
                Stub::FileWithContent("logs/app.log", "log"),
                Stub::FileWithContent("debug.log", "log"),
                Stub::FileWithContent("node_modules/react/index.js", "js"),
                Stub::FileWithContent(".vscode/settings.json", "json"),
                Stub::FileWithContent(".env", "env"),
                Stub::FileWithContent(".DS_Store", "store"),
                Stub::FileWithContent("temp.swp", "swap"),
                Stub::FileWithContent("src/main.rs", "rust"),
                Stub::FileWithContent("README.md", "readme"),
                Stub::FileWithContent("package.json", "json"),
                Stub::FileWithContent("docs/guide.md", "guide"),
                Stub::FileWithContent("tests/integration.rs", "test"),
                Stub::FileWithContent(".github/workflows/ci.yml", "yml"),
                Stub::FileWithContent(".gitignore", gitignore_content),
            ]);

            let gitignore_path = repo_path.join(".gitignore");

            let (gitignore, _) = Gitignore::new(&gitignore_path);
            let gitignores = vec![gitignore];

            // Test various patterns
            let test_cases = vec![
                // Should be ignored
                (repo_path.join("target").join("debug").join("main"), false),
                (repo_path.join("dist").join("bundle.js"), false),
                (repo_path.join("logs").join("app.log"), false),
                (repo_path.join("debug.log"), false),
                (
                    repo_path
                        .join("node_modules")
                        .join("react")
                        .join("index.js"),
                    false,
                ),
                (repo_path.join(".vscode").join("settings.json"), false),
                (repo_path.join(".env"), false),
                (repo_path.join(".DS_Store"), false),
                (repo_path.join("temp.swp"), false),
                // Should be included
                (repo_path.join("src").join("main.rs"), true),
                (repo_path.join("README.md"), true),
                (repo_path.join("package.json"), true),
                (repo_path.join("docs").join("guide.md"), true),
                (repo_path.join("tests").join("integration.rs"), true),
                (
                    repo_path.join(".github").join("workflows").join("ci.yml"),
                    true,
                ),
            ];

            for (path, should_include) in test_cases {
                let actual = !CurrentAppRepoMetadataModel::path_is_ignored(&path, &gitignores);
                assert_eq!(
                    actual, should_include,
                    "Path {path:?} - expected: {should_include}, actual: {actual}"
                );
            }
        });
    }

    #[test]
    fn test_git_directory_exclusion() {
        VirtualFS::test("git_exclusion_test", |dirs, mut fs| {
            let repo_path = dirs.tests();

            // Create .git directory and files using VirtualFS
            fs.mkdir(".git/objects").mkdir("src").with_files(vec![
                Stub::FileWithContent(".git/config", "config"),
                Stub::FileWithContent(".git/objects/abc123", "object"),
                Stub::FileWithContent("src/main.rs", "rust"),
            ]);

            let gitignores = vec![]; // Empty gitignore rules

            // .git directory and its contents should be excluded
            assert!(CurrentAppRepoMetadataModel::path_is_ignored(
                &repo_path.join(".git"),
                &gitignores
            ));
            assert!(CurrentAppRepoMetadataModel::path_is_ignored(
                &repo_path.join(".git").join("config"),
                &gitignores
            ));
            assert!(CurrentAppRepoMetadataModel::path_is_ignored(
                &repo_path.join(".git").join("objects").join("abc123"),
                &gitignores
            ));

            // Regular files should be included
            assert!(!CurrentAppRepoMetadataModel::path_is_ignored(
                &repo_path.join("src").join("main.rs"),
                &gitignores
            ));
        });
    }

    #[test]
    fn test_nested_gitignore_rules() {
        VirtualFS::test("nested_gitignore_test", |dirs, mut fs| {
            let repo_path = dirs.tests();

            // Create nested directory structure and files using VirtualFS
            fs.mkdir("frontend/dist")
                .mkdir("backend/target")
                .mkdir("frontend/src")
                .with_files(vec![
                    Stub::FileWithContent("frontend/dist/bundle.js", "js"),
                    Stub::FileWithContent("backend/target/binary", "bin"),
                    Stub::FileWithContent("frontend/src/main.ts", "ts"),
                    Stub::FileWithContent(".gitignore", "*/dist/\n*/target/"),
                    Stub::FileWithContent("frontend/.gitignore", "!dist/important.js"),
                ]);

            // Create gitignore objects
            let root_gitignore_path = repo_path.join(".gitignore");
            let frontend_gitignore_path = repo_path.join("frontend").join(".gitignore");

            let (root_gitignore, _) = Gitignore::new(&root_gitignore_path);
            let (frontend_gitignore, _) = Gitignore::new(&frontend_gitignore_path);
            let gitignores = vec![root_gitignore, frontend_gitignore];

            // Test that nested gitignore rules are respected
            assert!(CurrentAppRepoMetadataModel::path_is_ignored(
                &repo_path.join("frontend").join("dist").join("bundle.js"),
                &gitignores
            ));
            assert!(CurrentAppRepoMetadataModel::path_is_ignored(
                &repo_path.join("backend").join("target").join("binary"),
                &gitignores
            ));
            assert!(!CurrentAppRepoMetadataModel::path_is_ignored(
                &repo_path.join("frontend").join("src").join("main.ts"),
                &gitignores
            ));
        });
    }

    #[test]
    fn test_ensure_parent_directories_exist() {
        use crate::current_app_model::CurrentAppRepoMetadataModel;

        // Test case 1: Normal operation - creating nested parent directories
        let root_entry = Entry::Directory(DirectoryEntry {
            path: StandardizedPath::try_new("/test_repo").unwrap(),
            children: vec![Entry::Directory(DirectoryEntry {
                path: StandardizedPath::try_new("/test_repo/src").unwrap(),
                children: vec![],
                ignored: false,
                loaded: true,
            })],
            ignored: false,
            loaded: true,
        });
        let mut root = FileTreeEntry::from(root_entry);

        // Try to ensure parent directories exist for a deeply nested path
        CurrentAppRepoMetadataModel::ensure_parent_directories_exist(
            &mut root,
            &StandardizedPath::try_new("/test_repo/src/components/ui/forms").unwrap(),
        );

        // Verify that all intermediate directories were created
        let mut all_paths = Vec::new();
        collect_all_paths(&root, &mut all_paths);

        assert!(all_paths.contains(&StandardizedPath::try_new("/test_repo").unwrap()));
        assert!(all_paths.contains(&StandardizedPath::try_new("/test_repo/src").unwrap()));
        assert!(
            all_paths.contains(&StandardizedPath::try_new("/test_repo/src/components").unwrap())
        );
        assert!(
            all_paths.contains(&StandardizedPath::try_new("/test_repo/src/components/ui").unwrap())
        );
        assert!(all_paths
            .contains(&StandardizedPath::try_new("/test_repo/src/components/ui/forms").unwrap()));

        // Test case 2: Existing directories should not be recreated
        let initial_count = all_paths.len();
        CurrentAppRepoMetadataModel::ensure_parent_directories_exist(
            &mut root,
            &StandardizedPath::try_new("/test_repo/src/components/ui/forms").unwrap(),
        );

        let mut updated_paths = Vec::new();
        collect_all_paths(&root, &mut updated_paths);
        assert_eq!(
            initial_count,
            updated_paths.len(),
            "No new directories should be created when they already exist"
        );

        // Test case 3: Edge case - file exists where directory is expected
        // This tests the edge case documented in the function's comment
        let root_with_file_conflict_entry = Entry::Directory(DirectoryEntry {
            path: StandardizedPath::try_new("/test_repo").unwrap(),
            children: vec![
                // Create a file at the path where we'll try to create a directory
                Entry::File(FileMetadata::from_standardized(
                    StandardizedPath::try_new("/test_repo/conflicting_path").unwrap(),
                    false,
                )),
            ],
            ignored: false,
            loaded: true,
        });
        let mut root_with_file_conflict = FileTreeEntry::from(root_with_file_conflict_entry);

        // Try to create parent directories where a file already exists
        CurrentAppRepoMetadataModel::ensure_parent_directories_exist(
            &mut root_with_file_conflict,
            &StandardizedPath::try_new("/test_repo/conflicting_path/nested/deep").unwrap(),
        );

        // Verify that the function returned early and didn't corrupt the tree
        let mut conflict_paths = Vec::new();
        collect_all_paths(&root_with_file_conflict, &mut conflict_paths);

        // The function should detect the file conflict and return early without creating
        // any nested directories beyond the conflicting file.

        // Should still have the original file
        assert!(conflict_paths
            .contains(&StandardizedPath::try_new("/test_repo/conflicting_path").unwrap()));
        // Should NOT have created nested directories beyond the conflict
        assert!(!conflict_paths
            .contains(&StandardizedPath::try_new("/test_repo/conflicting_path/nested").unwrap()));
        assert!(!conflict_paths.contains(
            &StandardizedPath::try_new("/test_repo/conflicting_path/nested/deep").unwrap()
        ));

        // Verify the conflicting entry is still a file, not a directory
        let conflicting_entry = root_with_file_conflict
            .get(&StandardizedPath::try_new("/test_repo/conflicting_path").unwrap())
            .expect("Conflicting entry should exist");
        assert!(
            matches!(conflicting_entry, FileTreeEntryState::File(_)),
            "Conflicting entry should remain a file"
        );

        {
            // Test case 3b: File conflict at intermediate level
            let root_with_intermediate_conflict_entry = Entry::Directory(DirectoryEntry {
                path: StandardizedPath::try_new("/test_repo").unwrap(),
                children: vec![Entry::Directory(DirectoryEntry {
                    path: StandardizedPath::try_new("/test_repo/src").unwrap(),
                    children: vec![
                        // Create a file where we expect a directory
                        Entry::File(FileMetadata::from_standardized(
                            StandardizedPath::try_new("/test_repo/src/components").unwrap(),
                            false,
                        )),
                    ],
                    ignored: false,
                    loaded: true,
                })],
                ignored: false,
                loaded: true,
            });
            let mut root_with_intermediate_conflict =
                FileTreeEntry::from(root_with_intermediate_conflict_entry);

            // Try to create nested directories where an intermediate path has a file conflict
            CurrentAppRepoMetadataModel::ensure_parent_directories_exist(
                &mut root_with_intermediate_conflict,
                &StandardizedPath::try_new("/test_repo/src/components/ui/forms").unwrap(),
            );

            // Verify that the function handled the conflict properly
            let mut intermediate_conflict_paths = Vec::new();
            collect_all_paths(
                &root_with_intermediate_conflict,
                &mut intermediate_conflict_paths,
            );

            // Should still have the original file at components level
            assert!(intermediate_conflict_paths
                .contains(&StandardizedPath::try_new("/test_repo/src/components").unwrap()));

            // Should NOT have created deeper nested directories beyond the conflict
            assert!(!intermediate_conflict_paths
                .contains(&StandardizedPath::try_new("/test_repo/src/components/ui").unwrap()));
            assert!(!intermediate_conflict_paths.contains(
                &StandardizedPath::try_new("/test_repo/src/components/ui/forms").unwrap()
            ));

            // Verify the conflicting entry is still a file, not a directory
            let conflicting_entry = root_with_intermediate_conflict
                .get(&StandardizedPath::try_new("/test_repo/src/components").unwrap())
                .expect("Conflicting entry should exist");
            assert!(
                matches!(conflicting_entry, FileTreeEntryState::File(_)),
                "Conflicting entry should remain a file"
            );

            // Test case 4: Single level directory creation
            let simple_root_entry = Entry::Directory(DirectoryEntry {
                path: StandardizedPath::try_new("/simple").unwrap(),
                children: vec![],
                ignored: false,
                loaded: true,
            });
            let mut simple_root = FileTreeEntry::from(simple_root_entry);

            let simple_target = StandardizedPath::try_new("/simple/new_dir").unwrap();
            CurrentAppRepoMetadataModel::ensure_parent_directories_exist(
                &mut simple_root,
                &simple_target,
            );

            let mut simple_paths = Vec::new();
            collect_all_paths(&simple_root, &mut simple_paths);
            assert!(simple_paths.contains(&StandardizedPath::try_new("/simple/new_dir").unwrap()));

            // Test case 5: Target parent is the root itself (edge case)
            let root_target_entry = Entry::Directory(DirectoryEntry {
                path: StandardizedPath::try_new("/root").unwrap(),
                children: vec![],
                ignored: false,
                loaded: true,
            });
            let mut root_target = FileTreeEntry::from(root_target_entry);

            // This should not crash or create any new directories
            CurrentAppRepoMetadataModel::ensure_parent_directories_exist(
                &mut root_target,
                &StandardizedPath::try_new("/root").unwrap(),
            );

            let mut root_paths = Vec::new();
            collect_all_paths(&root_target, &mut root_paths);
            assert_eq!(root_paths.len(), 1); // Should only contain the root itself

            // Test case 6: Empty path handling
            let empty_root_entry = Entry::Directory(DirectoryEntry {
                path: StandardizedPath::try_new("/empty").unwrap(),
                children: vec![],
                ignored: false,
                loaded: true,
            });
            let mut empty_root = FileTreeEntry::from(empty_root_entry);

            // Test with a path that has no additional parents to create
            let same_level_target = StandardizedPath::try_new("/empty").unwrap();
            CurrentAppRepoMetadataModel::ensure_parent_directories_exist(
                &mut empty_root,
                &same_level_target,
            );

            let mut empty_paths = Vec::new();
            collect_all_paths(&empty_root, &mut empty_paths);
            assert_eq!(empty_paths.len(), 1); // Should still only contain the root
        }
    }

    /// Helper function to collect all paths in a file tree
    fn collect_all_paths(entry: &FileTreeEntry, paths: &mut Vec<StandardizedPath>) {
        let root_path = entry.root_directory().clone();
        collect_paths_recursive(entry, &root_path, paths);
    }

    fn collect_paths_recursive(
        entry: &FileTreeEntry,
        current_path: &StandardizedPath,
        paths: &mut Vec<StandardizedPath>,
    ) {
        paths.push(current_path.clone());
        if let Some(FileTreeEntryState::Directory(_)) = entry.get(current_path) {
            for child in entry.child_paths(current_path) {
                collect_paths_recursive(entry, child, paths);
            }
        }
    }

    #[test]
    fn test_canonicalized_path_functionality() {
        use warp_util::standardized_path::StandardizedPath;
        VirtualFS::test("canonicalized_path_test", |dirs, mut vfs| {
            let repo_path = dirs.tests();

            // Create a directory structure with symlinks
            vfs.mkdir("real_dir/subdir")
                .mkdir("other_dir")
                .with_files(vec![
                    Stub::FileWithContent("real_dir/file.txt", "content"),
                    Stub::FileWithContent("real_dir/subdir/nested.rs", "rust code"),
                ]);

            let real_dir = repo_path.join("real_dir");
            let symlink_dir = repo_path.join("symlinked_dir");
            let relative_path = repo_path.join("./real_dir");

            // Create a symlink to real_dir
            #[cfg(unix)]
            let symlink_created = std::os::unix::fs::symlink(&real_dir, &symlink_dir).is_ok();
            #[cfg(windows)]
            let symlink_created =
                std::os::windows::fs::symlink_dir(&real_dir, &symlink_dir).is_ok();

            if symlink_created {
                // Test that different path representations canonicalize to the same path
                let canonical_real = StandardizedPath::from_local_canonicalized(&real_dir).unwrap();
                let canonical_symlink =
                    StandardizedPath::from_local_canonicalized(&symlink_dir).unwrap();
                let canonical_relative =
                    StandardizedPath::from_local_canonicalized(&relative_path).unwrap();

                // All should point to the same canonical path
                assert_eq!(canonical_real, canonical_symlink);
                assert_eq!(canonical_real, canonical_relative);

                // Test that the canonical path is absolute and resolved
                let local = canonical_real.to_local_path().unwrap();
                assert!(local.is_absolute());
                assert!(!local.to_string_lossy().contains("./"));
            }

            // Test with various input types
            let path_buf = real_dir.clone();
            let path_ref = real_dir.as_path();

            let canonical_from_pathbuf =
                StandardizedPath::from_local_canonicalized(&path_buf).unwrap();
            let canonical_from_path = StandardizedPath::from_local_canonicalized(path_ref).unwrap();

            // All should be equal
            assert_eq!(canonical_from_pathbuf, canonical_from_path);

            // Test conversion to local path
            let canonical = StandardizedPath::from_local_canonicalized(&real_dir).unwrap();
            let local_path = canonical.to_local_path().unwrap();

            // Test internal consistency - compare with dunce-canonicalized version
            let expected_canonical = dunce::canonicalize(&real_dir).unwrap();
            assert_eq!(local_path, expected_canonical);

            // Test error handling for non-existent paths
            let nonexistent = repo_path.join("nonexistent");
            let result = StandardizedPath::from_local_canonicalized(&nonexistent);
            assert!(result.is_err());
        });
    }

    #[test]
    fn test_repository_operations_with_standardized_paths() {
        use warp_util::standardized_path::StandardizedPath;

        VirtualFS::test("repo_canonicalized_test", |dirs, mut vfs| {
            let test_root = dirs.tests();

            // Create a real repository directory
            vfs.mkdir("real_repo/src")
                .mkdir("other_location")
                .with_files(vec![
                    Stub::FileWithContent("real_repo/src/main.rs", "fn main() {}"),
                    Stub::FileWithContent("real_repo/.gitignore", "*.log\n/target/"),
                    Stub::FileWithContent("real_repo/README.md", "# Project"),
                ]);

            let real_repo = test_root.join("real_repo");
            let symlink_repo = test_root.join("symlinked_repo");
            let relative_repo = test_root.join("./real_repo");

            // Create symlink to the repo
            #[cfg(unix)]
            let symlink_created = std::os::unix::fs::symlink(&real_repo, &symlink_repo).is_ok();
            #[cfg(windows)]
            let symlink_created =
                std::os::windows::fs::symlink_dir(&real_repo, &symlink_repo).is_ok();

            if symlink_created {
                App::test((), |mut app| async move {
                    let repo_watcher = app.add_singleton_model(DirectoryWatcher::new);
                    let _detected_repo =
                        app.add_singleton_model(|_| DetectedRepositories::default());
                    let model_handle = app.add_model(CurrentAppRepoMetadataModel::new);

                    // Create file tree state for testing
                    let src_file =
                        Entry::File(FileMetadata::new(real_repo.join("src/main.rs"), false));
                    let readme_file =
                        Entry::File(FileMetadata::new(real_repo.join("README.md"), false));
                    let src_dir = Entry::Directory(DirectoryEntry {
                        path: StandardizedPath::try_from_local(&real_repo.join("src")).unwrap(),
                        children: vec![src_file],
                        ignored: false,
                        loaded: true,
                    });
                    let root = Entry::Directory(DirectoryEntry {
                        path: StandardizedPath::try_from_local(&real_repo).unwrap(),
                        children: vec![src_dir, readme_file],
                        ignored: false,
                        loaded: true,
                    });

                    let (gitignore, _) = Gitignore::new(real_repo.join(".gitignore"));
                    let repo_handle = repo_watcher.update(&mut app, |repo_watcher, ctx| {
                        repo_watcher
                            .add_directory(
                                StandardizedPath::from_local_canonicalized(&real_repo).unwrap(),
                                ctx,
                            )
                            .unwrap()
                    });
                    let state = FileTreeState::new(root, vec![gitignore], Some(repo_handle));

                    // Test adding repository using different path representations
                    model_handle.update(&mut app, |model, ctx| {
                        // Add using real path
                        let result1 = model.add_repository_internal(
                            StandardizedPath::from_local_canonicalized(&real_repo).unwrap(),
                            state.clone(),
                            ctx,
                        );
                        assert!(result1.is_ok());

                        // Try to add using symlink path - this should canonicalize to the same path
                        let result2 = model.add_repository_internal(
                            StandardizedPath::from_local_canonicalized(&symlink_repo).unwrap(),
                            state.clone(),
                            ctx,
                        );
                        assert!(result2.is_ok());

                        // Try to add using relative path
                        let result3 = model.add_repository_internal(
                            StandardizedPath::from_local_canonicalized(&relative_repo).unwrap(),
                            state.clone(),
                            ctx,
                        );
                        assert!(result3.is_ok());

                        // Verify that only one repository entry exists (all paths canonicalized to the same)
                        let canonical_path =
                            StandardizedPath::from_local_canonicalized(&real_repo).unwrap();
                        assert!(model.repositories.contains_key(&canonical_path));
                    });

                    // Test find_repository_for_path with different path formats
                    model_handle.read(&app, |model, _ctx| {
                        let file_in_repo = real_repo.join("src/main.rs");
                        let symlink_file = symlink_repo.join("src/main.rs");

                        let found_real = model.find_repository_for_path(&file_in_repo);
                        let found_symlink = model.find_repository_for_path(&symlink_file);

                        // Both should find the same repository
                        assert!(found_real.is_some());
                        assert!(found_symlink.is_some());
                        assert_eq!(found_real, found_symlink);
                    });
                });
            }
        });
    }

    #[test]
    fn test_standardized_path_edge_cases() {
        use warp_util::standardized_path::StandardizedPath;

        VirtualFS::test("canonicalized_edge_cases", |dirs, mut vfs| {
            let test_root = dirs.tests();

            // Create test files and directories
            vfs.mkdir("existing_dir")
                .with_files(vec![Stub::FileWithContent("existing_file.txt", "content")]);

            let existing_dir = test_root.join("existing_dir");
            let existing_file = test_root.join("existing_file.txt");
            let nonexistent = test_root.join("nonexistent");

            // Test successful canonicalization
            assert!(StandardizedPath::from_local_canonicalized(&existing_dir).is_ok());
            assert!(StandardizedPath::from_local_canonicalized(&existing_file).is_ok());

            // Test failed canonicalization
            assert!(StandardizedPath::from_local_canonicalized(&nonexistent).is_err());

            // Test equality and hashing
            let canonical1 = StandardizedPath::from_local_canonicalized(&existing_dir).unwrap();
            let canonical2 = StandardizedPath::from_local_canonicalized(&existing_dir).unwrap();

            assert_eq!(canonical1, canonical2);

            // Test that they can be used in HashMaps
            let mut map = std::collections::HashMap::new();
            map.insert(canonical1.clone(), "value1");
            assert_eq!(map.get(&canonical2), Some(&"value1"));

            // Test Debug trait
            let debug_str = format!("{canonical1:?}");
            assert!(debug_str.contains("StandardizedPath"));
        });
    }

    /// 回归测试:终端在 `~` 启动时,文件树需能列出家目录的一级子项。
    /// 家目录可被索引为 lazy-loaded 路径(仅跳过递归 watch,不再整体拒绝)。
    #[cfg(feature = "local_fs")]
    #[test]
    fn test_index_lazy_loaded_home_dir_succeeds() {
        let Some(home) = dirs::home_dir() else {
            // 无 $HOME(沙箱 CI)— guard 本就是 no-op,跳过。
            return;
        };
        App::test((), |mut app| async move {
            let model_handle = app.add_model(|_| CurrentAppRepoMetadataModel::new_for_test());
            let home_std = StandardizedPath::from_local_canonicalized(&home).unwrap();
            let home_std_clone = home_std.clone();
            model_handle.update(&mut app, |model, ctx| {
                let result = model.index_lazy_loaded_path(&home_std_clone, ctx);
                assert!(
                    result.is_ok(),
                    "index_lazy_loaded_path for home dir must succeed, got: {result:?}"
                );
            });
            model_handle.read(&app, |model, _ctx| {
                assert!(
                    model.is_lazy_loaded_path(&home_std),
                    "home dir must be tracked as a lazy-loaded path"
                );
                assert!(
                    model.has_repository(&home_std),
                    "home dir entry must be present in the repository map"
                );
            });
        });
    }

    #[cfg(all(feature = "local_fs", unix))]
    #[test]
    fn external_directory_symlink_events_project_into_lexical_namespace() {
        use crate::SymlinkTargetKind;
        use std::os::unix::fs::symlink;

        let temp = tempfile::TempDir::new().unwrap();
        let repo_root = temp.path().join("repo");
        let external_target = temp.path().join("external-target");
        let link_path = repo_root.join("mounted");
        std::fs::create_dir_all(&repo_root).unwrap();
        std::fs::create_dir_all(&external_target).unwrap();
        std::fs::write(external_target.join("existing.txt"), "existing").unwrap();
        symlink(&external_target, &link_path).unwrap();

        let mut files = Vec::new();
        let mut gitignores = Vec::new();
        let root = Entry::build_tree(
            &repo_root,
            &mut files,
            &mut gitignores,
            None,
            1,
            0,
            &IgnoredPathStrategy::Include,
        )
        .unwrap();
        let repo_std = StandardizedPath::try_from_local(&repo_root).unwrap();
        let link_std = StandardizedPath::try_from_local(&link_path).unwrap();
        let mut state = FileTreeState::new(root, gitignores, None);
        state
            .entry
            .load_at_path(&link_std, &mut state.gitignores)
            .unwrap();
        assert!(matches!(
            state.entry.get(&link_std),
            Some(FileTreeEntryState::Symlink(link))
                if link.target_kind == SymlinkTargetKind::Directory && link.loaded
        ));

        App::test((), |mut app| async move {
            let model_handle = app.add_model(|_| CurrentAppRepoMetadataModel::new_for_test());
            model_handle.update(&mut app, |model, ctx| {
                model
                    .repositories
                    .insert(repo_std.clone(), IndexedRepoState::Indexed(state));
                model.refresh_symlink_watch_mounts(&repo_std, ctx);

                let canonical_child = dunce::canonicalize(&external_target)
                    .unwrap()
                    .join("new.txt");
                let projections = model.project_watch_path(&canonical_child);
                assert!(
                    projections.iter().any(|projection| {
                        projection.repo_path == repo_std
                            && projection.path == link_path.join("new.txt")
                            && projection.mount_path.as_ref() == Some(&link_std)
                    }),
                    "mounts={:?} projections={projections:?}",
                    model.symlink_watch_mounts
                );
                assert_eq!(
                    model
                        .watch_path_owners
                        .values()
                        .map(HashMap::len)
                        .sum::<usize>(),
                    2,
                    "one loaded external directory link must own content and lifecycle watches"
                );

                std::fs::write(&canonical_child, "new").unwrap();
                let event = ::watcher::BulkFilesystemWatcherEvent {
                    added: [canonical_child].into_iter().collect(),
                    ..Default::default()
                };
                model.handle_watcher_event(&event, ctx);
            });

            let lexical_child =
                StandardizedPath::try_from_local(&link_path.join("new.txt")).unwrap();
            for _ in 0..40 {
                if model_handle.read(&app, |model, _ctx| {
                    matches!(
                        model.repositories.get(&repo_std),
                        Some(IndexedRepoState::Indexed(state)) if state.entry.contains(&lexical_child)
                    )
                }) {
                    return;
                }
                warpui::r#async::Timer::after(Duration::from_millis(25)).await;
            }
            panic!(
                "canonical target event was not projected to lexical link path {}",
                lexical_child
            );
        });
    }

    #[cfg(all(feature = "local_fs", unix))]
    #[test]
    fn external_symlink_chain_retarget_refreshes_mount_from_lexical_lifecycle_event() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::TempDir::new().unwrap();
        let repo_root = temp.path().join("repo");
        let alias_parent = temp.path().join("aliases");
        let first_target = temp.path().join("targets-a/mounted");
        let second_target = temp.path().join("targets-b/mounted");
        let alias_path = alias_parent.join("current");
        let link_path = repo_root.join("mounted");
        std::fs::create_dir_all(&repo_root).unwrap();
        std::fs::create_dir_all(&alias_parent).unwrap();
        std::fs::create_dir_all(&first_target).unwrap();
        std::fs::create_dir_all(&second_target).unwrap();
        symlink(&first_target, &alias_path).unwrap();
        symlink(&alias_path, &link_path).unwrap();

        let mut files = Vec::new();
        let mut gitignores = Vec::new();
        let root = Entry::build_tree(
            &repo_root,
            &mut files,
            &mut gitignores,
            None,
            1,
            0,
            &IgnoredPathStrategy::Include,
        )
        .unwrap();
        let repo_std = StandardizedPath::try_from_local(&repo_root).unwrap();
        let link_std = StandardizedPath::try_from_local(&link_path).unwrap();
        let alias_std = StandardizedPath::try_from_local(&alias_path).unwrap();
        let alias_parent_std = StandardizedPath::from_local_canonicalized(&alias_parent).unwrap();
        let second_target_std = StandardizedPath::from_local_canonicalized(&second_target).unwrap();
        let mut state = FileTreeState::new(root, gitignores, None);
        state
            .entry
            .load_at_path(&link_std, &mut state.gitignores)
            .unwrap();

        App::test((), |mut app| async move {
            let model_handle = app.add_model(|_| CurrentAppRepoMetadataModel::new_for_test());
            model_handle.update(&mut app, |model, ctx| {
                model
                    .repositories
                    .insert(repo_std.clone(), IndexedRepoState::Indexed(state));
                model.refresh_symlink_watch_mounts(&repo_std, ctx);

                assert_eq!(
                    model
                        .watch_path_owners
                        .get(&alias_parent_std)
                        .and_then(|owners| owners.values().copied().max()),
                    Some(WatchDepth::Direct),
                    "the raw alias parent must own a lifecycle watch"
                );
                assert!(model
                    .project_watch_path(&alias_path)
                    .iter()
                    .any(|projection| {
                        projection.repo_path == repo_std
                            && projection.path == link_path
                            && projection.mount_path.as_ref() == Some(&link_std)
                    }));

                std::fs::remove_file(&alias_path).unwrap();
                symlink(&second_target, &alias_path).unwrap();
                model.handle_watcher_event(
                    &::watcher::BulkFilesystemWatcherEvent {
                        modified: [alias_path.clone()].into_iter().collect(),
                        ..Default::default()
                    },
                    ctx,
                );
            });

            for _ in 0..40 {
                if model_handle.read(&app, |model, _ctx| {
                    model
                        .symlink_watch_mounts
                        .get(&repo_std)
                        .and_then(|mounts| mounts.get(&link_std))
                        .is_some_and(|mount| {
                            mount.target_path == second_target_std
                                && mount.lifecycle_targets.contains(&alias_std)
                        })
                }) {
                    return;
                }
                warpui::r#async::Timer::after(Duration::from_millis(25)).await;
            }
            panic!("retargeting an intermediate symlink did not rebuild the lexical mount");
        });
    }

    #[cfg(feature = "local_fs")]
    #[test]
    fn repository_watcher_batches_are_single_flight_fifo_per_repo() {
        let temp = tempfile::TempDir::new().unwrap();
        let repo_root = temp.path().join("repo");
        std::fs::create_dir_all(&repo_root).unwrap();

        let mut files = Vec::new();
        let mut gitignores = Vec::new();
        let root = Entry::build_tree(
            &repo_root,
            &mut files,
            &mut gitignores,
            None,
            1,
            0,
            &IgnoredPathStrategy::Include,
        )
        .unwrap();
        let repo_std = StandardizedPath::try_from_local(&repo_root).unwrap();
        let first_path = repo_root.join("first.txt");
        let second_path = repo_root.join("second.txt");
        let first_std = StandardizedPath::try_from_local(&first_path).unwrap();
        let second_std = StandardizedPath::try_from_local(&second_path).unwrap();
        std::fs::write(&first_path, "first").unwrap();
        std::fs::write(&second_path, "second").unwrap();
        let state = FileTreeState::new(root, gitignores, None);

        App::test((), |mut app| async move {
            let model_handle = app.add_model(|_| CurrentAppRepoMetadataModel::new_for_test());
            model_handle.update(&mut app, |model, ctx| {
                model
                    .repositories
                    .insert(repo_std.clone(), IndexedRepoState::Indexed(state));
                model.handle_watcher_event(
                    &::watcher::BulkFilesystemWatcherEvent {
                        added: [first_path].into_iter().collect(),
                        ..Default::default()
                    },
                    ctx,
                );
                model.handle_watcher_event(
                    &::watcher::BulkFilesystemWatcherEvent {
                        added: [second_path].into_iter().collect(),
                        ..Default::default()
                    },
                    ctx,
                );

                assert_eq!(model.repo_update_in_flight.len(), 1);
                assert_eq!(
                    model
                        .pending_repo_updates
                        .get(&repo_std)
                        .map(|updates| updates.len()),
                    Some(1),
                    "the second batch must wait behind the active batch for this repo"
                );
            });

            for _ in 0..40 {
                if model_handle.read(&app, |model, _ctx| {
                    model.repo_update_in_flight.is_empty()
                        && model.pending_repo_updates.is_empty()
                        && matches!(
                            model.repositories.get(&repo_std),
                            Some(IndexedRepoState::Indexed(state))
                                if state.entry.contains(&first_std)
                                    && state.entry.contains(&second_std)
                        )
                }) {
                    return;
                }
                warpui::r#async::Timer::after(Duration::from_millis(25)).await;
            }
            panic!("serialized repository watcher batches did not drain in FIFO order");
        });
    }

    #[cfg(all(feature = "local_fs", unix))]
    #[test]
    fn external_target_delete_and_recreate_preserves_lexical_link_identity() {
        use crate::SymlinkTargetKind;
        use std::os::unix::fs::symlink;

        let temp = tempfile::TempDir::new().unwrap();
        let repo_root = temp.path().join("repo");
        let external_target = temp.path().join("external-target");
        let link_path = repo_root.join("mounted");
        std::fs::create_dir_all(&repo_root).unwrap();
        std::fs::create_dir_all(&external_target).unwrap();
        symlink(&external_target, &link_path).unwrap();

        let mut files = Vec::new();
        let mut gitignores = Vec::new();
        let root = Entry::build_tree(
            &repo_root,
            &mut files,
            &mut gitignores,
            None,
            1,
            0,
            &IgnoredPathStrategy::Include,
        )
        .unwrap();
        let repo_std = StandardizedPath::try_from_local(&repo_root).unwrap();
        let link_std = StandardizedPath::try_from_local(&link_path).unwrap();
        let canonical_target = dunce::canonicalize(&external_target).unwrap();
        let state = FileTreeState::new(root, gitignores, None);

        App::test((), |mut app| async move {
            let model_handle = app.add_model(|_| CurrentAppRepoMetadataModel::new_for_test());
            model_handle.update(&mut app, |model, ctx| {
                model
                    .repositories
                    .insert(repo_std.clone(), IndexedRepoState::Indexed(state));
                model.refresh_symlink_watch_mounts(&repo_std, ctx);

                std::fs::remove_dir(&external_target).unwrap();
                let event = ::watcher::BulkFilesystemWatcherEvent {
                    deleted: [canonical_target.clone()].into_iter().collect(),
                    ..Default::default()
                };
                let projections = model.project_watch_path(&canonical_target);
                assert_eq!(projections.len(), 1, "projections={projections:?}");
                assert!(projections.iter().any(|projection| {
                    projection.repo_path == repo_std
                        && projection.path == link_path
                        && projection.mount_path.as_ref() == Some(&link_std)
                }));
                model.handle_watcher_event(&event, ctx);
            });

            for _ in 0..40 {
                if model_handle.read(&app, |model, _ctx| {
                    matches!(
                        model.repositories.get(&repo_std),
                        Some(IndexedRepoState::Indexed(state))
                            if matches!(state.entry.get(&link_std),
                                Some(FileTreeEntryState::Symlink(link))
                                    if link.target_kind == SymlinkTargetKind::Missing)
                    )
                }) {
                    break;
                }
                warpui::r#async::Timer::after(Duration::from_millis(25)).await;
            }
            model_handle.read(&app, |model, _ctx| {
                let Some(IndexedRepoState::Indexed(state)) = model.repositories.get(&repo_std)
                else {
                    panic!("repository must remain indexed");
                };
                let entry = state.entry.get(&link_std);
                assert!(
                    matches!(
                        entry,
                        Some(FileTreeEntryState::Symlink(link))
                            if link.target_kind == SymlinkTargetKind::Missing
                    ),
                    "entry={entry:?}"
                );
            });

            std::fs::create_dir(&external_target).unwrap();
            model_handle.update(&mut app, |model, ctx| {
                let event = ::watcher::BulkFilesystemWatcherEvent {
                    added: [canonical_target].into_iter().collect(),
                    ..Default::default()
                };
                model.handle_watcher_event(&event, ctx);
            });

            for _ in 0..40 {
                if model_handle.read(&app, |model, _ctx| {
                    matches!(
                        model.repositories.get(&repo_std),
                        Some(IndexedRepoState::Indexed(state))
                            if matches!(state.entry.get(&link_std),
                                Some(FileTreeEntryState::Symlink(link))
                                    if link.target_kind == SymlinkTargetKind::Directory)
                    )
                }) {
                    return;
                }
                warpui::r#async::Timer::after(Duration::from_millis(25)).await;
            }
            panic!("recreated external target did not restore the lexical symlink");
        });
    }

    #[cfg(all(feature = "local_fs", unix))]
    #[test]
    fn external_file_symlink_target_lifecycle_updates_kind_without_losing_link() {
        use crate::SymlinkTargetKind;
        use std::os::unix::fs::symlink;

        let temp = tempfile::TempDir::new().unwrap();
        let repo_root = temp.path().join("repo");
        let external_target = temp.path().join("external.txt");
        let link_path = repo_root.join("mounted.txt");
        std::fs::create_dir_all(&repo_root).unwrap();
        std::fs::write(&external_target, "first").unwrap();
        symlink(&external_target, &link_path).unwrap();

        let mut files = Vec::new();
        let mut gitignores = Vec::new();
        let root = Entry::build_tree(
            &repo_root,
            &mut files,
            &mut gitignores,
            None,
            1,
            0,
            &IgnoredPathStrategy::Include,
        )
        .unwrap();
        let repo_std = StandardizedPath::try_from_local(&repo_root).unwrap();
        let link_std = StandardizedPath::try_from_local(&link_path).unwrap();
        let canonical_target = dunce::canonicalize(&external_target).unwrap();
        let state = FileTreeState::new(root, gitignores, None);

        App::test((), |mut app| async move {
            let model_handle = app.add_model(|_| CurrentAppRepoMetadataModel::new_for_test());
            model_handle.update(&mut app, |model, ctx| {
                model
                    .repositories
                    .insert(repo_std.clone(), IndexedRepoState::Indexed(state));
                model.refresh_symlink_watch_mounts(&repo_std, ctx);
                std::fs::remove_file(&external_target).unwrap();
                model.handle_watcher_event(
                    &::watcher::BulkFilesystemWatcherEvent {
                        deleted: [canonical_target.clone()].into_iter().collect(),
                        ..Default::default()
                    },
                    ctx,
                );
            });

            for _ in 0..40 {
                if model_handle.read(&app, |model, _ctx| {
                    matches!(
                        model.repositories.get(&repo_std),
                        Some(IndexedRepoState::Indexed(state))
                            if matches!(state.entry.get(&link_std),
                                Some(FileTreeEntryState::Symlink(link))
                                    if link.target_kind == SymlinkTargetKind::Missing)
                    )
                }) {
                    break;
                }
                warpui::r#async::Timer::after(Duration::from_millis(25)).await;
            }

            std::fs::write(&external_target, "second").unwrap();
            model_handle.update(&mut app, |model, ctx| {
                model.handle_watcher_event(
                    &::watcher::BulkFilesystemWatcherEvent {
                        added: [canonical_target].into_iter().collect(),
                        ..Default::default()
                    },
                    ctx,
                );
            });

            for _ in 0..40 {
                if model_handle.read(&app, |model, _ctx| {
                    matches!(
                        model.repositories.get(&repo_std),
                        Some(IndexedRepoState::Indexed(state))
                            if matches!(state.entry.get(&link_std),
                                Some(FileTreeEntryState::Symlink(link))
                                    if link.target_kind == SymlinkTargetKind::File)
                    )
                }) {
                    return;
                }
                warpui::r#async::Timer::after(Duration::from_millis(25)).await;
            }
            panic!("recreated external file target did not restore file-link kind");
        });
    }

    #[cfg(all(feature = "local_fs", unix))]
    #[test]
    fn broken_external_symlink_target_creation_is_projected_without_manual_refresh() {
        use crate::SymlinkTargetKind;
        use std::os::unix::fs::symlink;

        let temp = tempfile::TempDir::new().unwrap();
        let repo_root = temp.path().join("repo");
        let external_parent = temp.path().join("external");
        let intermediate = external_parent.join("stage-one");
        let external_target = intermediate.join("created-later");
        let link_path = repo_root.join("mounted");
        std::fs::create_dir_all(&repo_root).unwrap();
        std::fs::create_dir_all(&external_parent).unwrap();
        symlink(&external_target, &link_path).unwrap();

        let mut files = Vec::new();
        let mut gitignores = Vec::new();
        let root = Entry::build_tree(
            &repo_root,
            &mut files,
            &mut gitignores,
            None,
            1,
            0,
            &IgnoredPathStrategy::Include,
        )
        .unwrap();
        let repo_std = StandardizedPath::try_from_local(&repo_root).unwrap();
        let link_std = StandardizedPath::try_from_local(&link_path).unwrap();
        let parent_std = StandardizedPath::from_local_canonicalized(&external_parent).unwrap();
        let projected_intermediate = parent_std.to_local_path().unwrap().join("stage-one");
        let projected_target = projected_intermediate.join("created-later");
        let intermediate_std = StandardizedPath::try_from_local(&projected_intermediate).unwrap();
        let state = FileTreeState::new(root, gitignores, None);

        App::test((), |mut app| async move {
            let model_handle = app.add_model(|_| CurrentAppRepoMetadataModel::new_for_test());
            model_handle.update(&mut app, |model, ctx| {
                model
                    .repositories
                    .insert(repo_std.clone(), IndexedRepoState::Indexed(state));
                model.refresh_symlink_watch_mounts(&repo_std, ctx);
                assert_eq!(
                    model
                        .watch_path_owners
                        .get(&parent_std)
                        .and_then(|owners| owners.values().copied().max()),
                    Some(WatchDepth::Direct),
                    "broken target must own a direct nearest-ancestor lifecycle watch"
                );
                assert!(model
                    .project_watch_path(&projected_intermediate)
                    .iter()
                    .any(|projection| {
                        projection.repo_path == repo_std
                            && projection.path == link_path
                            && projection.mount_path.as_ref() == Some(&link_std)
                    }));

                std::fs::create_dir(&intermediate).unwrap();
                let event = ::watcher::BulkFilesystemWatcherEvent {
                    added: [projected_intermediate].into_iter().collect(),
                    ..Default::default()
                };
                model.handle_watcher_event(&event, ctx);
            });

            for _ in 0..40 {
                if model_handle.read(&app, |model, _ctx| {
                    model
                        .watch_path_owners
                        .get(&intermediate_std)
                        .and_then(|owners| owners.values().copied().max())
                        == Some(WatchDepth::Direct)
                }) {
                    break;
                }
                warpui::r#async::Timer::after(Duration::from_millis(25)).await;
            }
            model_handle.read(&app, |model, _ctx| {
                assert_eq!(
                    model
                        .watch_path_owners
                        .get(&intermediate_std)
                        .and_then(|owners| owners.values().copied().max()),
                    Some(WatchDepth::Direct),
                    "lifecycle watch must advance to the newly-created nearest ancestor"
                );
            });

            std::fs::create_dir(&external_target).unwrap();
            model_handle.update(&mut app, |model, ctx| {
                let event = ::watcher::BulkFilesystemWatcherEvent {
                    added: [projected_target].into_iter().collect(),
                    ..Default::default()
                };
                model.handle_watcher_event(&event, ctx);
            });

            for _ in 0..40 {
                if model_handle.read(&app, |model, _ctx| {
                    matches!(
                        model.repositories.get(&repo_std),
                        Some(IndexedRepoState::Indexed(state))
                            if matches!(state.entry.get(&link_std),
                                Some(FileTreeEntryState::Symlink(link))
                                    if link.target_kind == SymlinkTargetKind::Directory)
                    )
                }) {
                    return;
                }
                warpui::r#async::Timer::after(Duration::from_millis(25)).await;
            }
            panic!("created external target did not refresh the broken lexical symlink");
        });
    }

    #[cfg(all(feature = "local_fs", unix))]
    #[test]
    fn symlink_mount_cleanup_releases_watch_owner_and_lexical_subtree() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::TempDir::new().unwrap();
        let repo_root = temp.path().join("repo");
        let external_target = temp.path().join("external-target");
        let link_path = repo_root.join("mounted");
        let lexical_child_path = link_path.join("child.txt");
        std::fs::create_dir_all(&repo_root).unwrap();
        std::fs::create_dir_all(&external_target).unwrap();
        std::fs::write(external_target.join("child.txt"), "child").unwrap();
        symlink(&external_target, &link_path).unwrap();

        let mut files = Vec::new();
        let mut gitignores = Vec::new();
        let root = Entry::build_tree(
            &repo_root,
            &mut files,
            &mut gitignores,
            None,
            1,
            0,
            &IgnoredPathStrategy::Include,
        )
        .unwrap();
        let repo_std = StandardizedPath::try_from_local(&repo_root).unwrap();
        let link_std = StandardizedPath::try_from_local(&link_path).unwrap();
        let lexical_child_std = StandardizedPath::try_from_local(&lexical_child_path).unwrap();
        let canonical_target =
            StandardizedPath::from_local_canonicalized(&external_target).unwrap();
        let mut state = FileTreeState::new(root, gitignores, None);
        state
            .entry
            .load_at_path(&link_std, &mut state.gitignores)
            .unwrap();
        assert!(state.entry.contains(&lexical_child_std));

        App::test((), |mut app| async move {
            let model_handle = app.add_model(|_| CurrentAppRepoMetadataModel::new_for_test());
            model_handle.update(&mut app, |model, ctx| {
                model
                    .repositories
                    .insert(repo_std.clone(), IndexedRepoState::Indexed(state));
                model.refresh_symlink_watch_mounts(&repo_std, ctx);
                assert_eq!(
                    model
                        .watch_path_owners
                        .get(&canonical_target)
                        .map(HashMap::len),
                    Some(1)
                );

                let Some(IndexedRepoState::Indexed(state)) = model.repositories.get_mut(&repo_std)
                else {
                    panic!("repository must remain indexed");
                };
                state.entry.remove(&link_std);
                assert!(!state.entry.contains(&lexical_child_std));
                model.refresh_symlink_watch_mounts(&repo_std, ctx);

                assert!(!model.symlink_watch_mounts.contains_key(&repo_std));
                assert!(
                    !model.watch_path_owners.contains_key(&canonical_target),
                    "removing the loaded lexical link must release its exact target watch"
                );
            });
        });
    }

    #[cfg(feature = "local_fs")]
    #[test]
    fn exact_watch_path_stays_registered_until_every_owner_releases_it() {
        let temp = tempfile::TempDir::new().unwrap();
        let repo_a = StandardizedPath::try_from_local(&temp.path().join("repo-a")).unwrap();
        let repo_b = StandardizedPath::try_from_local(&temp.path().join("repo-b")).unwrap();
        let link_a = StandardizedPath::try_from_local(&temp.path().join("repo-a/link-a")).unwrap();
        let link_b = StandardizedPath::try_from_local(&temp.path().join("repo-a/link-b")).unwrap();
        std::fs::create_dir_all(repo_a.to_local_path().unwrap()).unwrap();
        std::fs::create_dir_all(repo_b.to_local_path().unwrap()).unwrap();

        App::test((), |mut app| async move {
            let model_handle = app.add_model(|_| CurrentAppRepoMetadataModel::new_for_test());
            model_handle.update(&mut app, |model, ctx| {
                assert!(model.acquire_watch_path(
                    &repo_b,
                    WatchPathOwner::Repository {
                        repo_path: repo_b.clone(),
                    },
                    WatchDepth::Recursive,
                    ctx,
                ));
                model.acquire_watch_path(
                    &repo_b,
                    WatchPathOwner::SymlinkMount {
                        repo_path: repo_a.clone(),
                        lexical_path: link_a.clone(),
                    },
                    WatchDepth::Recursive,
                    ctx,
                );
                model.acquire_watch_path(
                    &repo_b,
                    WatchPathOwner::SymlinkMount {
                        repo_path: repo_a.clone(),
                        lexical_path: link_b.clone(),
                    },
                    WatchDepth::Direct,
                    ctx,
                );
                assert_eq!(
                    model.watch_path_owners.get(&repo_b).map(HashMap::len),
                    Some(3),
                    "repository root and two lexical mounts must share one exact watch root"
                );
                assert_eq!(
                    model
                        .watch_path_owners
                        .get(&repo_b)
                        .and_then(|owners| owners.values().copied().max()),
                    Some(WatchDepth::Recursive)
                );

                model.release_watch_path(
                    &repo_b,
                    &WatchPathOwner::SymlinkMount {
                        repo_path: repo_a.clone(),
                        lexical_path: link_a,
                    },
                    ctx,
                );
                assert_eq!(
                    model.watch_path_owners.get(&repo_b).map(HashMap::len),
                    Some(2)
                );
                model.release_watch_path(
                    &repo_b,
                    &WatchPathOwner::Repository {
                        repo_path: repo_b.clone(),
                    },
                    ctx,
                );
                assert_eq!(
                    model.watch_path_owners.get(&repo_b).map(HashMap::len),
                    Some(1),
                    "releasing the repository owner must not tear down link-b's watch"
                );
                assert_eq!(
                    model
                        .watch_path_owners
                        .get(&repo_b)
                        .and_then(|owners| owners.values().copied().max()),
                    Some(WatchDepth::Direct),
                    "effective depth must downgrade only after every recursive owner releases"
                );
                model.release_watch_path(
                    &repo_b,
                    &WatchPathOwner::SymlinkMount {
                        repo_path: repo_a,
                        lexical_path: link_b,
                    },
                    ctx,
                );
                assert!(
                    !model.watch_path_owners.contains_key(&repo_b),
                    "the exact watch root may unregister only after its final owner releases"
                );
            });
        });
    }

    #[cfg(feature = "local_fs")]
    #[test]
    fn repository_lookup_respects_path_component_boundaries() {
        let temp = tempfile::TempDir::new().unwrap();
        let repo_path = temp.path().join("repo");
        let sibling_path = temp.path().join("repo-sibling");
        std::fs::create_dir_all(&repo_path).unwrap();
        std::fs::create_dir_all(&sibling_path).unwrap();
        let repo_std = StandardizedPath::try_from_local(&repo_path).unwrap();
        let mut files = Vec::new();
        let root = Entry::build_tree(
            &repo_path,
            &mut files,
            &mut vec![],
            None,
            1,
            0,
            &IgnoredPathStrategy::Include,
        )
        .unwrap();

        App::test((), |mut app| async move {
            let model_handle = app.add_model(|_| CurrentAppRepoMetadataModel::new_for_test());
            model_handle.update(&mut app, |model, _ctx| {
                let state = FileTreeState::new_lazy_loaded(root);
                model
                    .repositories
                    .insert(repo_std.clone(), IndexedRepoState::Indexed(state));

                assert_eq!(
                    model.find_repository_for_path(&repo_path.join("child.txt")),
                    Some(repo_std.clone())
                );
                assert_eq!(
                    model.find_repository_for_path(&sibling_path.join("child.txt")),
                    None,
                    "string-prefix siblings must never be projected into the repository"
                );
            });
        });
    }

    #[cfg(all(feature = "local_fs", unix))]
    #[test]
    fn broken_symlink_rename_remains_visible_without_rescan() {
        use crate::SymlinkTargetKind;
        use std::os::unix::fs::symlink;

        let temp = tempfile::TempDir::new().unwrap();
        let repo_root = temp.path().join("repo");
        let old_path = repo_root.join("broken-link");
        let new_path = repo_root.join("renamed-broken-link");
        std::fs::create_dir_all(&repo_root).unwrap();
        symlink("missing-target", &old_path).unwrap();

        let mut files = Vec::new();
        let mut gitignores = Vec::new();
        let root = Entry::build_tree(
            &repo_root,
            &mut files,
            &mut gitignores,
            None,
            1,
            0,
            &IgnoredPathStrategy::Include,
        )
        .unwrap();
        let repo_std = StandardizedPath::try_from_local(&repo_root).unwrap();
        let old_std = StandardizedPath::try_from_local(&old_path).unwrap();
        let state = FileTreeState::new(root, gitignores, None);
        assert!(matches!(
            state.entry.get(&old_std),
            Some(FileTreeEntryState::Symlink(link))
                if link.target_kind == SymlinkTargetKind::Missing
        ));

        std::fs::rename(&old_path, &new_path).unwrap();
        let new_std = StandardizedPath::try_from_local(&new_path).unwrap();

        App::test((), |mut app| async move {
            let model_handle = app.add_model(|_| CurrentAppRepoMetadataModel::new_for_test());
            model_handle.update(&mut app, |model, ctx| {
                model
                    .repositories
                    .insert(repo_std.clone(), IndexedRepoState::Indexed(state));
                model.handle_watcher_event(
                    &::watcher::BulkFilesystemWatcherEvent {
                        added: [new_path].into_iter().collect(),
                        deleted: [old_path].into_iter().collect(),
                        ..Default::default()
                    },
                    ctx,
                );
            });

            for _ in 0..40 {
                if model_handle.read(&app, |model, _ctx| {
                    matches!(
                        model.repositories.get(&repo_std),
                        Some(IndexedRepoState::Indexed(state))
                            if !state.entry.contains(&old_std)
                                && matches!(
                                    state.entry.get(&new_std),
                                    Some(FileTreeEntryState::Symlink(link))
                                        if link.target_kind == SymlinkTargetKind::Missing
                                )
                    )
                }) {
                    return;
                }
                warpui::r#async::Timer::after(Duration::from_millis(25)).await;
            }
            panic!("broken symlink rename did not remain visible after the incremental update");
        });
    }
}

#[cfg(unix)]
#[test]
fn watcher_mutations_preserve_file_and_broken_symlink_identity() {
    use super::{CurrentAppRepoMetadataModel, FileTreeMutation, RepoUpdate};
    use crate::{Entry, SymlinkTargetKind};
    use std::os::unix::fs::symlink;

    let temp = tempfile::TempDir::new().unwrap();
    std::fs::write(temp.path().join("target.txt"), "target").unwrap();
    let file_link = temp.path().join("file-link");
    let broken_link = temp.path().join("broken-link");
    symlink("target.txt", &file_link).unwrap();
    symlink("missing-target", &broken_link).unwrap();

    let update = RepoUpdate {
        added: vec![file_link.clone(), broken_link.clone()],
        ..RepoUpdate::default()
    };
    let mutations = futures::executor::block_on(
        CurrentAppRepoMetadataModel::compute_file_tree_mutations(&update, &[]),
    );

    assert!(mutations.iter().any(|mutation| matches!(
        mutation,
        FileTreeMutation::AddEntry { path, entry: Entry::Symlink(link) }
            if path == &file_link && link.target_kind == SymlinkTargetKind::File
    )));
    assert!(mutations.iter().any(|mutation| matches!(
        mutation,
        FileTreeMutation::AddEntry { path, entry: Entry::Symlink(link) }
            if path == &broken_link && link.target_kind == SymlinkTargetKind::Missing
    )));
}
