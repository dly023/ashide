#![cfg_attr(not(feature = "local_fs"), allow(dead_code))]

use ignore::gitignore::Gitignore;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use thiserror::Error;
use warp_util::standardized_path::StandardizedPath;

/// Maximum file size allowed for treesitter parsing (3MB).
const MAX_FILE_SIZE: usize = 3 * 1000 * 1000;

/// Maximum number of files to load when lazy-loading a directory
pub const LAZY_LOAD_FILE_LIMIT: usize = 5000;

#[derive(Debug, Error)]
pub enum BuildTreeError {
    #[error("Repo size exceeded max file limit")]
    ExceededMaxFileLimit,
    #[error("File is ignored")]
    Ignored,
    #[error("IO error reading path.")]
    IOError(#[from] io::Error),
    #[error("Unsupported filesystem entry type")]
    UnsupportedFileType,
    #[error("Maximum directory depth exceeded")]
    MaxDepthExceeded,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IgnoredPathStrategy {
    /// Do not include any ignored files or folders
    Exclude,

    /// Lazy-load excluded directories
    IncludeLazy,

    /// Exclude all ignored files except for the ones in the given list
    IncludeOnly(Vec<String>),

    /// Add all of the ignored files into the tree
    Include,
}

/// Filesystem entry.
#[derive(Debug, Clone)]
pub enum Entry {
    File(FileMetadata),
    Directory(DirectoryEntry),
    Symlink(SymlinkEntry),
}

#[derive(Debug, Clone, Copy, Hash, Eq, PartialEq)]
pub enum SymlinkTargetKind {
    File,
    Directory,
    Other,
    Missing,
}

#[derive(Debug, Clone, Copy, Hash, Eq, PartialEq)]
pub struct FileId(usize);

impl FileId {
    /// Constructs a new globally-unique file ID.
    #[allow(clippy::new_without_default)]
    pub(crate) fn new() -> FileId {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
        let raw = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        FileId(raw)
    }
}

impl Entry {
    pub fn path(&self) -> &StandardizedPath {
        match self {
            Self::File(file) => &file.path,
            Self::Directory(directory) => &directory.path,
            Self::Symlink(symlink) => &symlink.path,
        }
    }

    pub fn loaded(&self) -> bool {
        match self {
            Self::File(_) => true,
            Self::Directory(directory) => directory.loaded,
            Self::Symlink(symlink) => symlink.loaded,
        }
    }

    pub fn ignored(&self) -> bool {
        match self {
            Self::File(file) => file.ignored,
            Self::Directory(directory) => directory.ignored,
            Self::Symlink(symlink) => symlink.ignored,
        }
    }

    /// Builds a tree of entries from a given path, handling gitignored files and directories.
    /// After max_depth is reached, all children are lazy-loaded to prevent deeply nested trees.
    /// IgnoredPathStrategy determines what happens when ignored files are encountered.
    pub fn build_tree(
        path: impl Into<PathBuf>,
        files: &mut Vec<FileMetadata>,
        gitignores: &mut Vec<Gitignore>,
        mut remaining_file_quota: Option<&mut usize>,
        max_depth: usize,
        current_depth: usize,
        ignored_path_strategy: &IgnoredPathStrategy,
    ) -> Result<Self, BuildTreeError> {
        let curr_path: PathBuf = path.into();
        let link_metadata = std::fs::symlink_metadata(&curr_path)?;
        let is_symlink = link_metadata.file_type().is_symlink();
        let symlink_target_kind = is_symlink.then(|| symlink_target_kind(&curr_path));
        let is_directory_like =
            link_metadata.is_dir() || symlink_target_kind == Some(SymlinkTargetKind::Directory);

        let gitignore_path = curr_path.join(".gitignore");
        if !is_symlink && gitignore_path.exists() {
            let (gitignore, _) = Gitignore::new(gitignore_path);
            gitignores.push(gitignore);
        }

        let path_is_ignored = matches_gitignores(
            &curr_path,
            is_directory_like,
            &*gitignores,
            true, /* check_ancestors */
        ) || is_git_internal_path(&curr_path);

        let mut lazy_load = current_depth >= max_depth;
        if path_is_ignored {
            match ignored_path_strategy {
                IgnoredPathStrategy::Exclude => return Err(BuildTreeError::Ignored),
                IgnoredPathStrategy::IncludeOnly(patterns) => {
                    if let Some(file_name) = curr_path.file_name().and_then(|name| name.to_str()) {
                        if !patterns.iter().any(|pattern| file_name == pattern) {
                            return Err(BuildTreeError::Ignored);
                        }
                    }
                }
                IgnoredPathStrategy::IncludeLazy => lazy_load = true,
                IgnoredPathStrategy::Include => {}
            }
        }

        if let Some(target_kind) = symlink_target_kind {
            if target_kind == SymlinkTargetKind::File {
                if let Some(remaining_file_quota) = remaining_file_quota.as_deref_mut() {
                    if *remaining_file_quota == 0 {
                        return Err(BuildTreeError::ExceededMaxFileLimit);
                    }
                    *remaining_file_quota -= 1;
                }
                // Keep file-symlink paths indexable (for example WARP.md) without
                // erasing their link identity in the tree model.
                files.push(FileMetadata::new(curr_path.clone(), path_is_ignored));
            }
            return Ok(Self::Symlink(SymlinkEntry {
                path: StandardizedPath::from_local_absolute_unchecked(&curr_path),
                target_kind,
                children: Vec::new(),
                ignored: path_is_ignored,
                loaded: target_kind != SymlinkTargetKind::Directory,
            }));
        }

        if link_metadata.is_dir() {
            if lazy_load {
                return Ok(Self::Directory(DirectoryEntry {
                    children: Vec::new(),
                    path: StandardizedPath::from_local_absolute_unchecked(&curr_path),
                    ignored: path_is_ignored,
                    loaded: false,
                }));
            }

            let children = build_directory_children(
                &curr_path,
                files,
                gitignores,
                remaining_file_quota.as_deref_mut(),
                max_depth,
                current_depth,
                ignored_path_strategy,
            )?;
            Ok(Self::Directory(DirectoryEntry {
                children,
                path: StandardizedPath::from_local_absolute_unchecked(&curr_path),
                ignored: path_is_ignored,
                loaded: true,
            }))
        } else if link_metadata.is_file() {
            if let Some(remaining_file_quota) = remaining_file_quota {
                if *remaining_file_quota == 0 {
                    return Err(BuildTreeError::ExceededMaxFileLimit);
                }
                *remaining_file_quota -= 1;
            }
            let metadata = FileMetadata::new(curr_path, path_is_ignored);
            files.push(metadata.clone());
            Ok(Self::File(metadata))
        } else {
            Err(BuildTreeError::UnsupportedFileType)
        }
    }

    /// Finds an entry based on path
    pub fn find_mut(&mut self, path: &Path) -> Option<&mut Entry> {
        let std_path = StandardizedPath::try_from_local(path).ok()?;
        self.find_mut_by_std_path(&std_path)
    }

    fn find_mut_by_std_path(&mut self, path: &StandardizedPath) -> Option<&mut Entry> {
        if self.path() == path {
            return Some(self);
        }

        let (entry_path, children) = match self {
            Self::Directory(directory) => (&directory.path, &mut directory.children),
            Self::Symlink(symlink) if symlink.target_kind == SymlinkTargetKind::Directory => {
                (&symlink.path, &mut symlink.children)
            }
            Self::File(_) | Self::Symlink(_) => return None,
        };
        {
            if !path.starts_with(entry_path) {
                // Target is not descendant of directory.
                return None;
            }

            for child in children.iter_mut() {
                if let Some(entry) = child.find_mut_by_std_path(path) {
                    return Some(entry);
                }
            }
        }

        None
    }

    /// Loads an unloaded directory
    pub fn load(&mut self, gitignores: &mut Vec<Gitignore>) -> Result<(), BuildTreeError> {
        let (path, target_is_symlink_directory) = match self {
            Self::Directory(directory) => (directory.path.clone(), false),
            Self::Symlink(symlink) if symlink.target_kind == SymlinkTargetKind::Directory => {
                (symlink.path.clone(), true)
            }
            Self::File(_) | Self::Symlink(_) => return Ok(()),
        };

        let mut remaining_file_quota = LAZY_LOAD_FILE_LIMIT;
        let mut files = Vec::new();
        let children = build_directory_children(
            &path.to_local_path_lossy(),
            &mut files,
            gitignores,
            Some(&mut remaining_file_quota),
            1,
            0,
            &IgnoredPathStrategy::Include,
        )?;

        match self {
            Self::Directory(directory) => {
                directory.children = children;
                directory.loaded = true;
            }
            Self::Symlink(symlink) if target_is_symlink_directory => {
                symlink.children = children;
                symlink.loaded = true;
            }
            Self::File(_) | Self::Symlink(_) => {}
        }
        Ok(())
    }

    /// Removes the entry corresponding to the given target path, if any.
    pub fn remove(&mut self, target_path: &Path) -> Option<FileMetadata> {
        let std_path = StandardizedPath::try_from_local(target_path).ok()?;
        self.remove_by_std_path(&std_path)
    }

    fn remove_by_std_path(&mut self, target_path: &StandardizedPath) -> Option<FileMetadata> {
        let (entry_path, children) = match self {
            Self::Directory(directory) => (&directory.path, &mut directory.children),
            Self::Symlink(symlink) if symlink.target_kind == SymlinkTargetKind::Directory => {
                (&symlink.path, &mut symlink.children)
            }
            Self::File(_) | Self::Symlink(_) => return None,
        };
        if !target_path.starts_with(entry_path) {
            // Target is not descendant of directory.
            return None;
        }
        for (index, child) in children.iter_mut().enumerate() {
            if child.path() == target_path {
                // If the child's path is the target path, remove the child.
                return match children.remove(index) {
                    Entry::Directory(_) | Entry::Symlink(_) => None,
                    Entry::File(metadata) => Some(metadata),
                };
            } else if target_path.starts_with(child.path()) {
                // Child is a descendant of the target path, so recurse.
                return child.remove_by_std_path(target_path);
            }
        }

        log::debug!("target path not found under the current directory node");
        None
    }
}

fn build_directory_children(
    path: &Path,
    files: &mut Vec<FileMetadata>,
    gitignores: &mut Vec<Gitignore>,
    mut remaining_file_quota: Option<&mut usize>,
    max_depth: usize,
    current_depth: usize,
    ignored_path_strategy: &IgnoredPathStrategy,
) -> Result<Vec<Entry>, BuildTreeError> {
    let mut children = Vec::new();
    for directory_entry in std::fs::read_dir(path)? {
        if remaining_file_quota
            .as_ref()
            .is_some_and(|quota| **quota < children.len())
        {
            return Err(BuildTreeError::ExceededMaxFileLimit);
        }

        let Ok(directory_entry) = directory_entry else {
            continue;
        };
        // Keep lexical namespace identity. Canonicalizing here rewrites
        // `/link/child` into `/target/child`, which breaks expansion keys,
        // watcher updates, selection, and mutations.
        match Entry::build_tree(
            directory_entry.path(),
            files,
            gitignores,
            remaining_file_quota.as_deref_mut(),
            max_depth,
            current_depth + 1,
            ignored_path_strategy,
        ) {
            Ok(entry) => children.push(entry),
            Err(BuildTreeError::ExceededMaxFileLimit) => {
                return Err(BuildTreeError::ExceededMaxFileLimit);
            }
            Err(_) => {}
        }
    }
    Ok(children)
}

fn symlink_target_kind(path: &Path) -> SymlinkTargetKind {
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => SymlinkTargetKind::File,
        Ok(metadata) if metadata.is_dir() => {
            let Ok(target) = dunce::canonicalize(path) else {
                return SymlinkTargetKind::Other;
            };
            let creates_lexical_cycle = path.ancestors().skip(1).any(|ancestor| {
                dunce::canonicalize(ancestor)
                    .is_ok_and(|canonical_ancestor| canonical_ancestor.starts_with(&target))
            });
            if creates_lexical_cycle {
                // A direct ancestor link and a multi-link cycle have the same failure mode:
                // each expansion creates a new lexical namespace for a canonical directory
                // already present above it (`a/b/a/b/...`). Keep the link visible but
                // deliberately non-navigable.
                SymlinkTargetKind::Other
            } else {
                SymlinkTargetKind::Directory
            }
        }
        Ok(_) => SymlinkTargetKind::Other,
        Err(error) if error.kind() == io::ErrorKind::NotFound => SymlinkTargetKind::Missing,
        Err(_) => SymlinkTargetKind::Other,
    }
}

pub fn is_git_internal_path(path: &Path) -> bool {
    path.components().any(|component| {
        if let Component::Normal(name) = component {
            name == ".git"
        } else {
            false
        }
    })
}

/// Returns true if a path matches any of the gitignores.
///
/// For example, if the directory `/target` is ignored:
/// - If `check_ancestors` is true, then `/target/debug` will match.
/// - If `check_ancestors` is false, then `/target/debug` will not match.
pub fn matches_gitignores(
    path: &Path,
    is_dir: bool,
    gitignores: &[Gitignore],
    check_ancestors: bool,
) -> bool {
    gitignores.iter().any(|gitignore| {
        if let Ok(relative_path) = path.strip_prefix(gitignore.path()) {
            // `matched_path_or_any_parents` panics if the path has a root.
            // If not on windows, we allow paths with a root if the gitignore path is empty (since this denotes a global gitignore).
            if relative_path.has_root() && (cfg!(windows) || gitignore.path() != Path::new("")) {
                return false;
            }

            if check_ancestors {
                gitignore
                    .matched_path_or_any_parents(relative_path, is_dir)
                    .is_ignore()
            } else {
                gitignore.matched(relative_path, is_dir).is_ignore()
            }
        } else {
            false
        }
    })
}

/// Returns the path components after `.git` in a git-internal path,
/// skipping the worktree indirection (`.git/worktrees/<name>/…`) if present.
/// Returns `None` if the path has no `.git` component or nothing follows it.
fn git_suffix_components(path: &Path) -> Option<Vec<Component<'_>>> {
    let components: Vec<_> = path.components().collect();
    let git_index = components.iter().position(|c| c.as_os_str() == ".git")?;

    let after_git = &components[git_index + 1..];
    if after_git.is_empty() {
        return None;
    }

    // For worktrees the layout is `.git/worktrees/<name>/…`.
    // Skip the `worktrees/<name>` prefix so callers see the same
    // logical structure as a normal repo.
    if after_git.first().map(|c| c.as_os_str()) == Some(std::ffi::OsStr::new("worktrees"))
        && after_git.len() >= 3
    {
        // after_git[0] = "worktrees", [1] = <name>, [2..] = actual content
        return Some(after_git[2..].to_vec());
    }

    Some(after_git.to_vec())
}

/// Given a path like `.../repo/.git/worktrees/foo/HEAD`, returns
/// `.../repo/.git/worktrees/foo`. Returns `None` for non-worktree paths.
pub(crate) fn extract_worktree_git_dir(path: &Path) -> Option<PathBuf> {
    let components: Vec<_> = path.components().collect();
    let git_index = components.iter().position(|c| c.as_os_str() == ".git")?;
    let after_git = &components[git_index + 1..];
    if after_git.len() >= 3
        && after_git
            .first()
            .map(|c| c.as_os_str() == "worktrees")
            .unwrap_or(false)
    {
        // Rebuild: everything up to and including .git/worktrees/<name>
        Some(components[..git_index + 3].iter().collect())
    } else {
        None
    }
}

/// Returns `true` for shared ref paths that live directly in the common
/// `.git` directory and should be broadcast to all repos sharing it.
/// Currently this means `.git/refs/heads/*` (not under `.git/worktrees/`).
pub(crate) fn is_shared_git_ref(path: &Path) -> bool {
    if extract_worktree_git_dir(path).is_some() {
        return false;
    }
    let components: Vec<_> = path.components().collect();
    let Some(git_index) = components.iter().position(|c| c.as_os_str() == ".git") else {
        return false;
    };
    let after_git = &components[git_index + 1..];
    after_git
        .first()
        .map(|c| c.as_os_str() == "refs")
        .unwrap_or(false)
        && after_git
            .get(1)
            .map(|c| c.as_os_str() == "heads")
            .unwrap_or(false)
}

/// Returns true for `.git/HEAD` and `.git/refs/heads/*`
/// (and their worktree equivalents `.git/worktrees/*/HEAD`, etc.).
pub(crate) fn is_commit_related_git_file(path: &Path) -> bool {
    let Some(suffix) = git_suffix_components(path) else {
        return false;
    };
    match suffix.first().map(|c| c.as_os_str()) {
        Some(name) if name == "HEAD" => true,
        Some(name) if name == "refs" => {
            suffix.get(1).map(|c| c.as_os_str()) == Some(std::ffi::OsStr::new("heads"))
        }
        _ => false,
    }
}

/// Returns true for `.git/index.lock`
/// (and its worktree equivalent `.git/worktrees/*/index.lock`).
pub(crate) fn is_index_lock_file(path: &Path) -> bool {
    let Some(suffix) = git_suffix_components(path) else {
        return false;
    };
    suffix.len() == 1 && suffix[0].as_os_str() == "index.lock"
}

/// Determines if a git-related path should be ignored by the filesystem watcher.
///
/// Uses an allowlist approach: only commit-related files (HEAD, refs/heads/*)
/// and the index lock file are allowed through. Everything else inside `.git/`
/// is ignored.
pub fn should_ignore_git_path(path: &Path) -> bool {
    if !is_git_internal_path(path) {
        return false; // Not a git path, don't ignore
    }
    // Ignore everything inside .git/ except the allowlisted patterns.
    !is_commit_related_git_file(path) && !is_index_lock_file(path)
}

pub fn path_passes_filters(path: &Path, gitignores: &[Gitignore]) -> bool {
    let to_check_path = if path.exists() {
        match dunce::canonicalize(path) {
            Ok(canonical_path) => canonical_path,
            Err(_) => return false,
        }
    } else {
        path.to_path_buf()
    };

    !matches_gitignores(
        &to_check_path,
        to_check_path.is_dir(),
        gitignores,
        true, /* check_ancestors */
    ) && !should_ignore_git_path(&to_check_path)
}

/// Determines whether a file should be parsed by a treesitter query. For now the main criteria is it shouldn't
/// exceed the given file size limit.
pub fn is_file_parsable(path: &Path) -> Result<bool, io::Error> {
    std::fs::metadata(path).map(|metadata| (metadata.len() as usize) < MAX_FILE_SIZE)
}

pub fn gitignores_for_directory(directory_path: &Path) -> Vec<Gitignore> {
    let mut gitignores = Vec::new();
    let gitignore_path = directory_path.join(".gitignore");
    if gitignore_path.exists() {
        let (gitignore, _) = Gitignore::new(&gitignore_path);
        gitignores.push(gitignore);
    }
    let (global_gitignore, _) = Gitignore::global();
    if !global_gitignore.is_empty() {
        gitignores.push(global_gitignore);
    }
    gitignores
}

#[derive(Debug, Clone)]
pub struct FileMetadata {
    /// Absolute path to the file.
    pub path: StandardizedPath,
    pub file_id: FileId,
    pub extension: Option<String>,
    pub ignored: bool,
}

impl FileMetadata {
    pub fn new(path: PathBuf, ignored: bool) -> Self {
        let path_extension = path.extension().and_then(|extension| extension.to_str());
        let file_id = FileId::new();
        let std_path = StandardizedPath::from_local_absolute_unchecked(&path);
        Self {
            file_id,
            extension: path_extension.map(str::to_string),
            path: std_path,
            ignored,
        }
    }

    /// Construct from a [`StandardizedPath`] directly, without filesystem I/O.
    pub fn from_standardized(path: StandardizedPath, ignored: bool) -> Self {
        let file_id = FileId::new();
        let extension = path.extension().map(|s| s.to_owned());
        Self {
            file_id,
            extension,
            path,
            ignored,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SymlinkEntry {
    /// Absolute lexical link path. It must never be replaced by the canonical target path.
    pub path: StandardizedPath,
    pub target_kind: SymlinkTargetKind,
    pub children: Vec<Entry>,
    pub ignored: bool,
    pub loaded: bool,
}

#[derive(Debug, Clone)]
pub struct DirectoryEntry {
    /// Absolute path to the directory.
    pub path: StandardizedPath,
    pub children: Vec<Entry>,
    pub ignored: bool,
    pub loaded: bool,
}

impl DirectoryEntry {
    pub fn find_or_insert_child(&mut self, target_path: &Path) -> Option<&mut Entry> {
        let std_path = StandardizedPath::try_from_local(target_path).ok()?;

        // First, try to find the child's position
        if let Some(index) = self
            .children
            .iter()
            .position(|child| *child.path() == std_path)
        {
            // Child exists, return a mutable reference to it
            return Some(&mut self.children[index]);
        }

        // Child not found, create new entry if the path is valid
        let new_entry = if target_path.is_symlink() {
            let target_kind = symlink_target_kind(target_path);
            Entry::Symlink(SymlinkEntry {
                path: std_path,
                target_kind,
                children: Vec::new(),
                ignored: false,
                loaded: target_kind != SymlinkTargetKind::Directory,
            })
        } else if target_path.is_dir() {
            Entry::Directory(DirectoryEntry {
                children: vec![],
                path: std_path,
                loaded: false,
                ignored: false,
            })
        } else if target_path.is_file() {
            Entry::File(FileMetadata {
                path: std_path.clone(),
                file_id: FileId::new(),
                extension: std_path.extension().map(|s| s.to_owned()),
                ignored: false,
            })
        } else {
            // Cannot insert child since target_path is neither a file or a directory.
            return None;
        };

        // Insert the new entry and return a mutable reference to it
        self.children.push(new_entry);
        self.children.last_mut()
    }

    /// Similar to find_or_insert_child but specifically for creating directory entries.
    /// This is used when we know the path should be a directory (e.g., when ensuring parent directories exist).
    pub fn find_or_insert_directory(&mut self, target_path: &Path) -> Option<&mut Entry> {
        let std_path = StandardizedPath::try_from_local(target_path).ok()?;

        // First, try to find the child's position
        if let Some(index) = self
            .children
            .iter()
            .position(|child| *child.path() == std_path)
        {
            // Child exists, return a mutable reference to it
            return Some(&mut self.children[index]);
        }

        // Child not found, create new directory entry
        let new_entry = Entry::Directory(DirectoryEntry {
            children: vec![],
            path: std_path,
            ignored: false,
            loaded: false,
        });

        // Insert the new entry and return a mutable reference to it
        self.children.push(new_entry);
        self.children.last_mut()
    }
}

#[cfg(test)]
#[path = "entry_test.rs"]
mod tests;
