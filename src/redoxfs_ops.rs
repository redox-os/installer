//! Helper functions for transaction-based RedoxFS operations.
//!
//! This module provides utilities for creating files, directories, and symlinks
//! directly in RedoxFS using the Transaction API, without requiring FUSE.

use anyhow::{bail, Result};
use redoxfs::{Disk, Node, Transaction, TreeData, TreePtr};
use std::path::Path;

/// Navigate to the parent directory of the given path, creating intermediate directories as needed.
/// Returns the TreePtr of the parent directory.
/// If a path component is a symlink, it will be followed (symlink must point to a directory).
pub fn ensure_parent_dirs<D: Disk>(
    tx: &mut Transaction<D>,
    path: &Path,
    ctime: u64,
    ctime_nsec: u32,
) -> Result<TreePtr<Node>> {
    let mut current_ptr = TreePtr::root();

    // Get the parent path, if any
    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => return Ok(current_ptr), // No parent needed, return root
    };

    for component in parent.components() {
        let name = match component {
            std::path::Component::Normal(s) => s.to_str().ok_or_else(|| {
                anyhow::anyhow!("Invalid UTF-8 in path component: {:?}", s)
            })?,
            std::path::Component::RootDir => continue,
            _ => continue, // Skip other components like . or ..
        };

        // Try to find existing directory
        match tx.find_node(current_ptr, name) {
            Ok(tree_data) => {
                let node = tree_data.data();
                // Check if it's a symlink and follow it
                if node.mode() & Node::MODE_TYPE == Node::MODE_SYMLINK {
                    // Read symlink target
                    let node_ptr = tree_data.ptr();
                    let size = node.size();
                    let mut target_buf = vec![0u8; size as usize];
                    let _bytes_read = tx.read_node(node_ptr, 0, &mut target_buf, ctime, ctime_nsec)
                        .map_err(|e| anyhow::anyhow!("Failed to read symlink '{}': {}", name, e))?;
                    let target = std::str::from_utf8(&target_buf)
                        .map_err(|e| anyhow::anyhow!("Symlink '{}' target is not valid UTF-8: {}", name, e))?;

                    // Resolve symlink target relative to current directory
                    // For absolute symlinks, start from root
                    // For relative symlinks, navigate from current position
                    if target.starts_with('/') {
                        // Absolute symlink - resolve from root
                        current_ptr = TreePtr::root();
                        for part in target.trim_start_matches('/').split('/') {
                            if part.is_empty() || part == "." {
                                continue;
                            }
                            if part == ".." {
                                // Go up - not supported for now, bail
                                bail!("Symlink with .. not supported: {}", target);
                            }
                            match tx.find_node(current_ptr, part) {
                                Ok(part_data) => {
                                    current_ptr = part_data.ptr();
                                }
                                Err(err) if err.errno == syscall::ENOENT => {
                                    // Create the missing directory
                                    let mode = Node::MODE_DIR | 0o755;
                                    let mut new_tree = tx.create_node(current_ptr, part, mode, ctime, ctime_nsec)
                                        .map_err(|e| anyhow::anyhow!("Failed to create directory '{}': {}", part, e))?;
                                    let new_ptr = new_tree.ptr();
                                    new_tree.data_mut().set_uid(0);
                                    new_tree.data_mut().set_gid(0);
                                    tx.sync_tree(new_tree)?;
                                    current_ptr = new_ptr;
                                }
                                Err(err) => {
                                    bail!("Failed to find node '{}' in symlink target: {}", part, err);
                                }
                            }
                        }
                    } else {
                        // Relative symlink - resolve from current directory
                        for part in target.split('/') {
                            if part.is_empty() || part == "." {
                                continue;
                            }
                            if part == ".." {
                                bail!("Symlink with .. not supported: {}", target);
                            }
                            match tx.find_node(current_ptr, part) {
                                Ok(part_data) => {
                                    current_ptr = part_data.ptr();
                                }
                                Err(err) if err.errno == syscall::ENOENT => {
                                    let mode = Node::MODE_DIR | 0o755;
                                    let mut new_tree = tx.create_node(current_ptr, part, mode, ctime, ctime_nsec)
                                        .map_err(|e| anyhow::anyhow!("Failed to create directory '{}': {}", part, e))?;
                                    let new_ptr = new_tree.ptr();
                                    new_tree.data_mut().set_uid(0);
                                    new_tree.data_mut().set_gid(0);
                                    tx.sync_tree(new_tree)?;
                                    current_ptr = new_ptr;
                                }
                                Err(err) => {
                                    bail!("Failed to find node '{}' in symlink target: {}", part, err);
                                }
                            }
                        }
                    }
                } else {
                    current_ptr = tree_data.ptr();
                }
            }
            Err(err) if err.errno == syscall::ENOENT => {
                // Create directory with default permissions 0o755
                let mode = Node::MODE_DIR | 0o755;
                let mut tree_data = tx
                    .create_node(current_ptr, name, mode, ctime, ctime_nsec)
                    .map_err(|e| anyhow::anyhow!("Failed to create directory '{}': {}", name, e))?;

                // Get the pointer before syncing (sync_tree consumes tree_data)
                let new_ptr = tree_data.ptr();

                // Set default uid/gid (root) and sync
                tree_data.data_mut().set_uid(0);
                tree_data.data_mut().set_gid(0);
                tx.sync_tree(tree_data)
                    .map_err(|e| anyhow::anyhow!("Failed to sync directory '{}': {}", name, e))?;

                current_ptr = new_ptr;
            }
            Err(err) => {
                bail!("Failed to find node '{}': {}", name, err);
            }
        }
    }

    Ok(current_ptr)
}

/// Create a file with the given content, permissions, and ownership.
/// Returns the TreePtr of the created file.
pub fn create_file<D: Disk>(
    tx: &mut Transaction<D>,
    parent_ptr: TreePtr<Node>,
    name: &str,
    content: &[u8],
    mode: u16,
    uid: u32,
    gid: u32,
    mtime: u64,
    mtime_nsec: u32,
) -> Result<TreePtr<Node>> {
    // Create the file node
    let file_mode = Node::MODE_FILE | (mode & Node::MODE_PERM);
    let mut tree_data = tx
        .create_node(parent_ptr, name, file_mode, mtime, mtime_nsec)
        .map_err(|e| anyhow::anyhow!("Failed to create file '{}': {}", name, e))?;

    // Get the pointer before syncing (sync_tree consumes tree_data)
    let node_ptr = tree_data.ptr();

    // Set ownership and sync
    tree_data.data_mut().set_uid(uid);
    tree_data.data_mut().set_gid(gid);
    tx.sync_tree(tree_data)
        .map_err(|e| anyhow::anyhow!("Failed to sync file metadata '{}': {}", name, e))?;

    // Write content if not empty
    if !content.is_empty() {
        tx.write_node(node_ptr, 0, content, mtime, mtime_nsec as u32)
            .map_err(|e| anyhow::anyhow!("Failed to write file content '{}': {}", name, e))?;
    }

    Ok(node_ptr)
}

/// Create a directory with the given permissions and ownership.
/// Returns the TreePtr of the created directory.
pub fn create_directory<D: Disk>(
    tx: &mut Transaction<D>,
    parent_ptr: TreePtr<Node>,
    name: &str,
    mode: u16,
    uid: u32,
    gid: u32,
    ctime: u64,
    ctime_nsec: u32,
) -> Result<TreePtr<Node>> {
    let dir_mode = Node::MODE_DIR | (mode & Node::MODE_PERM);
    let mut tree_data = tx
        .create_node(parent_ptr, name, dir_mode, ctime, ctime_nsec)
        .map_err(|e| anyhow::anyhow!("Failed to create directory '{}': {}", name, e))?;

    // Get the pointer before syncing (sync_tree consumes tree_data)
    let node_ptr = tree_data.ptr();

    // Set ownership and sync
    tree_data.data_mut().set_uid(uid);
    tree_data.data_mut().set_gid(gid);
    tx.sync_tree(tree_data)
        .map_err(|e| anyhow::anyhow!("Failed to sync directory '{}': {}", name, e))?;

    Ok(node_ptr)
}

/// Create a symlink pointing to the given target.
/// Returns the TreePtr of the created symlink.
pub fn create_symlink<D: Disk>(
    tx: &mut Transaction<D>,
    parent_ptr: TreePtr<Node>,
    name: &str,
    target: &str,
    ctime: u64,
    ctime_nsec: u32,
) -> Result<TreePtr<Node>> {
    // Create symlink node - symlinks typically have mode 0o777
    let symlink_mode = Node::MODE_SYMLINK | 0o777;
    let mut tree_data = tx
        .create_node(parent_ptr, name, symlink_mode, ctime, ctime_nsec)
        .map_err(|e| anyhow::anyhow!("Failed to create symlink '{}': {}", name, e))?;

    // Get the pointer before syncing (sync_tree consumes tree_data)
    let node_ptr = tree_data.ptr();

    // Set default ownership (root) and sync
    tree_data.data_mut().set_uid(0);
    tree_data.data_mut().set_gid(0);
    tx.sync_tree(tree_data)
        .map_err(|e| anyhow::anyhow!("Failed to sync symlink '{}': {}", name, e))?;

    // Write the symlink target as file content
    tx.write_node(node_ptr, 0, target.as_bytes(), ctime, ctime_nsec as u32)
        .map_err(|e| anyhow::anyhow!("Failed to write symlink target '{}': {}", name, e))?;

    Ok(node_ptr)
}

/// Find a node by path, returning None if it doesn't exist.
pub fn find_node_by_path<D: Disk>(
    tx: &mut Transaction<D>,
    path: &Path,
) -> Result<Option<TreeData<Node>>> {
    let mut current_ptr = TreePtr::root();

    for component in path.components() {
        let name = match component {
            std::path::Component::Normal(s) => s.to_str().ok_or_else(|| {
                anyhow::anyhow!("Invalid UTF-8 in path component: {:?}", s)
            })?,
            std::path::Component::RootDir => continue,
            _ => continue,
        };

        match tx.find_node(current_ptr, name) {
            Ok(tree_data) => {
                current_ptr = tree_data.ptr();
            }
            Err(err) if err.errno == syscall::ENOENT => {
                return Ok(None);
            }
            Err(err) => {
                bail!("Failed to find node '{}': {}", name, err);
            }
        }
    }

    // Get the final node
    Ok(Some(
        tx.read_tree(current_ptr)
            .map_err(|e| anyhow::anyhow!("Failed to read node: {}", e))?,
    ))
}

/// Create a file or directory at the given path, creating parent directories as needed.
/// Returns the TreePtr of the created node.
pub fn create_at_path<D: Disk>(
    tx: &mut Transaction<D>,
    path: &Path,
    is_directory: bool,
    is_symlink: bool,
    content: &[u8],
    mode: u16,
    uid: u32,
    gid: u32,
    ctime: u64,
    ctime_nsec: u32,
) -> Result<TreePtr<Node>> {
    // Ensure parent directories exist
    let parent_ptr = ensure_parent_dirs(tx, path, ctime, ctime_nsec)?;

    // Get the filename
    let name = path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("Path has no filename: {:?}", path))?
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Invalid UTF-8 in filename"))?;

    if is_directory {
        // Check if directory already exists (may have been created as parent of earlier files)
        match tx.find_node(parent_ptr, name) {
            Ok(tree_data) => {
                // Directory already exists, just return its pointer
                // TODO: optionally update mode/uid/gid if needed
                Ok(tree_data.ptr())
            }
            Err(err) if err.errno == syscall::ENOENT => {
                // Directory doesn't exist, create it
                create_directory(tx, parent_ptr, name, mode, uid, gid, ctime, ctime_nsec)
            }
            Err(err) => {
                bail!("Failed to check if directory '{}' exists: {}", name, err);
            }
        }
    } else if is_symlink {
        // Check if symlink already exists
        match tx.find_node(parent_ptr, name) {
            Ok(tree_data) => {
                // Symlink already exists, skip
                Ok(tree_data.ptr())
            }
            Err(err) if err.errno == syscall::ENOENT => {
                let target = std::str::from_utf8(content)
                    .map_err(|e| anyhow::anyhow!("Symlink target is not valid UTF-8: {}", e))?;
                create_symlink(tx, parent_ptr, name, target, ctime, ctime_nsec)
            }
            Err(err) => {
                bail!("Failed to check if symlink '{}' exists: {}", name, err);
            }
        }
    } else {
        // Check if file already exists
        match tx.find_node(parent_ptr, name) {
            Ok(tree_data) => {
                // File already exists, skip
                // TODO: optionally overwrite or update content
                Ok(tree_data.ptr())
            }
            Err(err) if err.errno == syscall::ENOENT => {
                create_file(
                    tx, parent_ptr, name, content, mode, uid, gid, ctime, ctime_nsec,
                )
            }
            Err(err) => {
                bail!("Failed to check if file '{}' exists: {}", name, err);
            }
        }
    }
}

/// Write content to a file in chunks, useful for large files.
pub fn write_file_chunked<D: Disk>(
    tx: &mut Transaction<D>,
    node_ptr: TreePtr<Node>,
    content: &[u8],
    mtime: u64,
    mtime_nsec: u32,
) -> Result<()> {
    const CHUNK_SIZE: usize = 64 * 1024; // 64KB chunks

    let mut offset = 0u64;
    while offset < content.len() as u64 {
        let end = std::cmp::min(offset as usize + CHUNK_SIZE, content.len());
        let chunk = &content[offset as usize..end];

        tx.write_node(node_ptr, offset, chunk, mtime, mtime_nsec as u32)
            .map_err(|e| anyhow::anyhow!("Failed to write chunk at offset {}: {}", offset, e))?;

        offset = end as u64;
    }

    Ok(())
}

/// Extract a pkgar package directly into RedoxFS using the transaction API.
pub fn extract_pkgar_to_tx<D: Disk, E: std::error::Error>(
    tx: &mut Transaction<D>,
    package: &mut impl pkgar_core::PackageSrc<Err = E>,
    ctime: u64,
    ctime_nsec: u32,
) -> Result<()> {
    let entries = package
        .read_entries()
        .map_err(|e| anyhow::anyhow!("Failed to read package entries: {}", e))?;

    for entry in entries {
        let path_bytes = entry.path_bytes();
        let path_str = std::str::from_utf8(path_bytes)
            .map_err(|e| anyhow::anyhow!("Invalid UTF-8 in entry path: {}", e))?;
        let path = Path::new(path_str);

        let mode = entry
            .mode()
            .map_err(|e| anyhow::anyhow!("Invalid mode for entry '{}': {}", path_str, e))?;

        // Ensure parent directories exist
        let parent_ptr = ensure_parent_dirs(tx, path, ctime, ctime_nsec)?;

        let name = match path.file_name() {
            Some(n) => n.to_str().ok_or_else(|| {
                anyhow::anyhow!("Invalid UTF-8 in filename for entry '{}'", path_str)
            })?,
            None => continue, // Skip entries without a filename (shouldn't happen)
        };

        let kind = mode.kind();

        if kind.contains(pkgar_core::Mode::SYMLINK) {
            // Read symlink target from package (Entry is Copy)
            let mut target = vec![0u8; entry.size() as usize];
            package
                .read_entry(entry, 0, &mut target)
                .map_err(|e| anyhow::anyhow!("Failed to read symlink target '{}': {}", path_str, e))?;

            let target_str = std::str::from_utf8(&target)
                .map_err(|e| anyhow::anyhow!("Symlink target '{}' is not valid UTF-8: {}", path_str, e))?;

            println!("Extracting symlink {} -> {}", path.display(), target_str);
            create_symlink(tx, parent_ptr, name, target_str, ctime, ctime_nsec)?;
        } else if kind.contains(pkgar_core::Mode::FILE) {
            // Extract regular file
            let perm_bits = mode.perm().bits() as u16;

            println!("Extracting file {} ({} bytes)", path.display(), entry.size());

            // Create file node
            let file_mode = Node::MODE_FILE | (perm_bits & Node::MODE_PERM);
            let mut tree_data = tx
                .create_node(parent_ptr, name, file_mode, ctime, ctime_nsec)
                .map_err(|e| anyhow::anyhow!("Failed to create file '{}': {}", path_str, e))?;

            let node_ptr = tree_data.ptr();

            // Set default ownership (root:root for packages)
            tree_data.data_mut().set_uid(0);
            tree_data.data_mut().set_gid(0);
            tx.sync_tree(tree_data)
                .map_err(|e| anyhow::anyhow!("Failed to sync file '{}': {}", path_str, e))?;

            // Write file content in chunks
            const CHUNK_SIZE: usize = 64 * 1024;
            let mut offset: usize = 0;
            let file_size = entry.size() as usize;
            let mut buf = vec![0u8; CHUNK_SIZE];

            while offset < file_size {
                let to_read = std::cmp::min(CHUNK_SIZE, file_size - offset);
                let buf_slice = &mut buf[..to_read];
                package
                    .read_entry(entry, offset, buf_slice)
                    .map_err(|e| anyhow::anyhow!("Failed to read file '{}' at offset {}: {}", path_str, offset, e))?;

                tx.write_node(node_ptr, offset as u64, buf_slice, ctime, ctime_nsec as u32)
                    .map_err(|e| anyhow::anyhow!("Failed to write file '{}' at offset {}: {}", path_str, offset, e))?;

                offset += to_read;
            }
        }
        // Note: pkgar doesn't have MODE_DIR - directories are implicit from file paths
    }

    Ok(())
}
