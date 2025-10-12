use std::{
    ffi::OsStr,
    path::{Component, Path},
    time::Duration,
};

use redoxfs::{Disk, FileSystem, Node, TreePtr};

use crate::Result;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct FileConfig {
    path: String,
    data: String,
    #[serde(default)]
    symlink: bool,
    #[serde(default)]
    directory: bool,
    mode: Option<u32>,
    uid: Option<u32>,
    gid: Option<u32>,
    #[serde(default)]
    recursive_chown: bool,
    #[serde(default)]
    postinstall: bool,
}

impl FileConfig {
    pub fn new_file(path: impl Into<String>, data: impl Into<String>) -> FileConfig {
        FileConfig {
            path: path.into(),
            data: data.into(),
            ..Default::default()
        }
    }

    pub fn new_directory(path: impl Into<String>) -> FileConfig {
        FileConfig {
            path: path.into(),
            data: String::new(),
            directory: true,
            ..Default::default()
        }
    }

    pub fn with_mod(&mut self, mode: u32, uid: u32, gid: u32) -> &mut FileConfig {
        self.mode = Some(mode);
        self.uid = Some(uid);
        self.gid = Some(gid);
        self
    }

    pub fn with_recursive_mod(&mut self, mode: u32, uid: u32, gid: u32) -> &mut FileConfig {
        self.with_mod(mode, uid, gid);
        self.recursive_chown = true;
        self
    }

    pub(crate) fn create<D: Disk>(&self, filesystem: &mut FileSystem<D>) -> Result<()> {
        let ctime = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?;

        if self.directory {
            self.create_directory(filesystem, ctime)?;
        } else {
            self.create_file(filesystem, ctime)?;
        };

        Ok(())
    }

    fn create_file<D: Disk>(&self, filesystem: &mut FileSystem<D>, ctime: Duration) -> Result<()> {
        let filename = if let Component::Normal(val) = Path::new(&self.path)
            .components()
            .next_back()
            .expect("Expected at least one element in path-components iterator")
        {
            val
        } else {
            panic!("Expected final path-component of non-directory FileConfig to be a filename");
        };
        let parent_id = self.create_dir_all(
            filesystem,
            ctime,
            Path::new(&self.path)
                .parent()
                .expect("Expected file to have parent directory"),
        )?;
        let mode = if self.symlink {
            Node::MODE_SYMLINK | 0o0777 & Node::MODE_PERM
        } else {
            Node::MODE_FILE | self.mode.unwrap_or(0o0644) as u16 & Node::MODE_PERM
        };
        let node = filesystem.tx(|tx| {
            tx.create_node(
                TreePtr::<Node>::new(parent_id),
                filename.to_str().expect(&format!(
                    "Expected filename to be valid utf-8: {:?}",
                    filename
                )),
                mode,
                ctime.as_secs(),
                ctime.subsec_nanos(),
            )
        })?;
        filesystem.tx(|tx| {
            tx.write_node(
                TreePtr::<Node>::new(node.id()),
                0,
                self.data.as_bytes(),
                ctime.as_secs(),
                ctime.subsec_nanos(),
            )
        })?;

        self.apply_owners(filesystem, parent_id, filename)?;

        Ok(())
    }

    fn create_directory<D: Disk>(
        &self,
        filesystem: &mut FileSystem<D>,
        ctime: Duration,
    ) -> Result<()> {
        let parent_id = self.create_dir_all(
            filesystem,
            ctime,
            Path::new(&self.path)
                .parent()
                .expect("Expected directory to have parent directory"),
        )?;
        let dirname = if let Component::Normal(dir) = Path::new(&self.path)
            .components()
            .next_back()
            .expect("Safe as iterator has length greater than 1")
        {
            dir
        } else {
            OsStr::new("/")
        };
        filesystem.tx(|tx| {
            tx.create_node(
                TreePtr::<Node>::new(parent_id),
                dirname.to_str().expect(&format!(
                    "Expected dirname io be valid utf-8: {:?}",
                    dirname
                )),
                Node::MODE_DIR | self.mode.unwrap_or(0o0755) as u16 & Node::MODE_PERM,
                ctime.as_secs(),
                ctime.subsec_nanos(),
            )
        })?;
        self.apply_owners(filesystem, parent_id, dirname)?;

        Ok(())
    }

    fn create_dir_all<D: Disk>(
        &self,
        filesystem: &mut FileSystem<D>,
        ctime: Duration,
        path: &Path,
    ) -> Result<u32> {
        let mut parent_id = TreePtr::<Node>::root().id();

        for dir in path.components() {
            let parent_ptr = TreePtr::<Node>::new(parent_id);
            match dir {
                Component::RootDir => continue,
                Component::Normal(subdir) => {
                    let subdir = subdir.to_str().expect(&format!(
                        "Expected subdir name to be valid utf-8: {:?}",
                        subdir
                    ));
                    let node = filesystem.tx(|tx| match tx.find_node(parent_ptr, subdir) {
                        Ok(node) => Ok(node),
                        Err(_) => tx.create_node(
                            parent_ptr,
                            subdir,
                            Node::MODE_DIR | 0o0755 & Node::MODE_PERM,
                            ctime.as_secs(),
                            ctime.subsec_nanos(),
                        ),
                    })?;
                    parent_id = node.id();
                }
                _ => todo!(),
            }
        }

        Ok(parent_id)
    }

    fn apply_owners<D: Disk>(
        &self,
        filesystem: &mut FileSystem<D>,
        parent_id: u32,
        component_name: &OsStr,
    ) -> Result<()> {
        let mut node = filesystem
            .tx(|tx| {
                tx.find_node(
                    TreePtr::<Node>::new(parent_id),
                    component_name.to_str().expect(&format!(
                        "Expected component name to be valid utf-8: {:?}",
                        component_name
                    )),
                )
            })
            .unwrap();
        node.data_mut().set_uid(self.uid.unwrap_or(!0));
        node.data_mut().set_gid(self.gid.unwrap_or(!0));
        let mode = if self.directory {
            Node::MODE_DIR | self.mode.unwrap_or(0o0755) as u16 & Node::MODE_PERM
        } else {
            let type_mask = if self.symlink {
                Node::MODE_SYMLINK
            } else {
                Node::MODE_FILE
            };
            let default_mode = if self.symlink { 0o0777 } else { 0o0644 };
            type_mask | self.mode.unwrap_or(default_mode) as u16 & Node::MODE_PERM
        };
        node.data_mut().set_mode(mode);

        if self.recursive_chown {
            self.recursive_apply_owners_and_perms(filesystem, node.id())
                .expect("Expected to be able to recursively apply mode and owners");
        }

        filesystem.tx(|tx| tx.sync_tree(node))?;
        Ok(())
    }

    fn recursive_apply_owners_and_perms<D: Disk>(
        &self,
        filesystem: &mut FileSystem<D>,
        id: u32,
    ) -> Result<()> {
        let node_ptr = TreePtr::<Node>::new(id);
        let mut node = filesystem
            .tx(|tx| tx.read_tree(node_ptr))
            .expect("Expected to be able to get node data");

        if let Some(uid) = self.uid {
            if node.data().uid() != uid {
                node.data_mut().set_uid(uid);
            }
        }

        if let Some(gid) = self.gid {
            if node.data().gid() != gid {
                node.data_mut().set_gid(gid);
            }
        }

        if let Some(mode) = self.mode {
            if node.data().mode() & Node::MODE_PERM != (mode as u16) & Node::MODE_PERM {
                let new_mode =
                    node.data().mode() & Node::MODE_TYPE | (mode as u16) & Node::MODE_PERM;
                node.data_mut().set_mode(new_mode);
            }
        }

        let is_file = node.data().is_file();
        filesystem.tx(|tx| tx.sync_tree(node))?;

        if is_file {
            return Ok(());
        }

        let mut children = Vec::new();
        filesystem
            .tx(|tx| tx.child_nodes(TreePtr::<Node>::new(id), &mut children))
            .expect("Expected to be able to retrieve child nodes");
        for child in children {
            self.recursive_apply_owners_and_perms(filesystem, child.node_ptr().id())
                .expect("Expected to be able to apply owners and perms");
        }

        Ok(())
    }
}

#[cfg(test)]
mod test {
    use std::path::{Component, Path};

    use redoxfs::{Disk, FileSystem, Node, TreePtr, BLOCK_SIZE};

    use super::FileConfig;

    const MOCK_DISK_SIZE: u64 = 1024 * 1024 * 1024;

    struct MockDisk(Vec<u8>);

    impl Disk for MockDisk {
        fn size(&mut self) -> syscall::Result<u64> {
            Ok(MOCK_DISK_SIZE)
        }

        unsafe fn read_at(&mut self, block: u64, buffer: &mut [u8]) -> syscall::Result<usize> {
            buffer.copy_from_slice(
                &self.0[(block * BLOCK_SIZE) as usize..((block + 1) * BLOCK_SIZE) as usize],
            );
            Ok(BLOCK_SIZE as usize)
        }

        unsafe fn write_at(&mut self, block: u64, buffer: &[u8]) -> syscall::Result<usize> {
            self.0[(block * BLOCK_SIZE) as usize..((block + 1) * BLOCK_SIZE) as usize]
                .copy_from_slice(buffer);
            Ok(BLOCK_SIZE as usize)
        }
    }

    fn create_mock_filesystem() -> FileSystem<MockDisk> {
        let disk = MockDisk(vec![0; MOCK_DISK_SIZE as usize]);
        let ctime = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap();
        FileSystem::create(disk, None, ctime.as_secs(), ctime.subsec_nanos()).unwrap()
    }

    #[test]
    fn write_file_node_in_existent_dir() {
        let mut filesystem = create_mock_filesystem();
        let filename = "foo.txt";
        let dirname = "root";
        let parent_dirpath = format!("/{dirname}");
        let filepath = format!("{parent_dirpath}/{filename}");
        let data = "Hello, world!";
        FileConfig::new_directory(parent_dirpath)
            .create(&mut filesystem)
            .unwrap();
        FileConfig::new_file(filepath, data)
            .create(&mut filesystem)
            .unwrap();
        let dir_node = filesystem
            .tx(|tx| tx.find_node(TreePtr::<Node>::root(), dirname))
            .unwrap();
        let file_node = filesystem
            .tx(|tx| tx.find_node(TreePtr::<Node>::new(dir_node.id()), filename))
            .unwrap();
        assert!(file_node.data().is_file());
        let mut buf = [0; 13];
        filesystem
            .tx(|tx| tx.read_node(TreePtr::<Node>::new(file_node.id()), 0, &mut buf, 1, 0))
            .unwrap();
        assert_eq!(&buf, data.as_bytes());
    }

    #[test]
    fn write_file_node_parents_if_non_existent() {
        let mut filesystem = create_mock_filesystem();
        let filename = "foo.txt";
        let dirname = "dir";
        let subdirname = "subdir";
        let filepath = format!("/{dirname}/{subdirname}/{filename}");
        let data = "Hello, world!";
        FileConfig::new_file(filepath, data)
            .create(&mut filesystem)
            .unwrap();
        let dir_node = filesystem
            .tx(|tx| tx.find_node(TreePtr::<Node>::root(), dirname))
            .unwrap();
        assert!(dir_node.data().is_dir());
        let subdir_node = filesystem
            .tx(|tx| tx.find_node(TreePtr::<Node>::new(dir_node.id()), subdirname))
            .unwrap();
        assert!(subdir_node.data().is_dir());
        let file_node = filesystem
            .tx(|tx| tx.find_node(TreePtr::<Node>::new(subdir_node.id()), filename))
            .unwrap();
        let mut buf = [0; 13];
        filesystem
            .tx(|tx| tx.read_node(TreePtr::<Node>::new(file_node.id()), 0, &mut buf, 1, 0))
            .unwrap();
        assert_eq!(&buf, data.as_bytes());
    }

    #[test]
    fn write_symlink_file_node() {
        let mut filesystem = create_mock_filesystem();
        let filename = "bin";
        let filepath = format!("/{filename}");
        let data = "user/bin";
        let mut file_config = FileConfig::new_file(filepath, data);
        file_config.symlink = true;
        file_config.create(&mut filesystem).unwrap();
        let node = filesystem
            .tx(|tx| tx.find_node(TreePtr::<Node>::root(), filename))
            .unwrap();
        assert_eq!(node.data().mode(), Node::MODE_SYMLINK | 0o0777);
    }

    #[test]
    fn default_file_node_perms() {
        let mut filesystem = create_mock_filesystem();
        let filename = "foo.txt";
        let filepath = format!("/{filename}");
        FileConfig::new_file(filepath, "")
            .create(&mut filesystem)
            .unwrap();
        let node = filesystem
            .tx(|tx| tx.find_node(TreePtr::<Node>::root(), filename))
            .unwrap();
        assert_eq!(
            node.data().mode() & Node::MODE_PERM,
            0o0644 & Node::MODE_PERM
        );
    }

    #[test]
    fn default_file_node_owners() {
        let mut filesystem = create_mock_filesystem();
        let filename = "foo.txt";
        let filepath = format!("/{filename}");
        FileConfig::new_file(filepath, "")
            .create(&mut filesystem)
            .unwrap();
        let node = filesystem
            .tx(|tx| tx.find_node(TreePtr::<Node>::root(), filename))
            .unwrap();
        assert_eq!(node.data().uid(), !0);
        assert_eq!(node.data().gid(), !0);
    }

    #[test]
    fn create_all_parents_of_dir_node() {
        let mut filesystem = create_mock_filesystem();
        let dirpath = "/dir/subdir/subsubdir";
        FileConfig::new_directory(dirpath)
            .create(&mut filesystem)
            .unwrap();
        let mut parent_id = TreePtr::<Node>::root().id();
        for dir in Path::new(dirpath).components() {
            let parent_ptr = TreePtr::<Node>::new(parent_id);
            match dir {
                Component::RootDir => continue,
                Component::Normal(subdir) => {
                    let node = filesystem
                        .tx(|tx| tx.find_node(parent_ptr, subdir.to_str().unwrap()))
                        .unwrap();
                    assert!(node.data().is_dir());
                    parent_id = node.id();
                }
                _ => panic!(),
            }
        }
    }

    #[test]
    fn create_subdir_within_existing_dir_doesnt_fail() {
        let mut filesystem = create_mock_filesystem();
        let dirpath = "/dir";
        let subdirpath = "/dir/subdir";
        FileConfig::new_directory(dirpath)
            .create(&mut filesystem)
            .unwrap();
        FileConfig::new_directory(subdirpath)
            .create(&mut filesystem)
            .unwrap();
        let mut parent_id = TreePtr::<Node>::root().id();
        for dir in Path::new(subdirpath).components() {
            let parent_ptr = TreePtr::<Node>::new(parent_id);
            match dir {
                Component::RootDir => continue,
                Component::Normal(subdir) => {
                    let node = filesystem
                        .tx(|tx| tx.find_node(parent_ptr, subdir.to_str().unwrap()))
                        .unwrap();
                    assert!(node.data().is_dir());
                    parent_id = node.id();
                }
                _ => panic!(),
            }
        }
    }

    #[test]
    fn default_dir_node_perms() {
        let mut filesystem = create_mock_filesystem();
        let dirname = "root";
        let dirpath = format!("/{dirname}");
        FileConfig::new_directory(dirpath)
            .create(&mut filesystem)
            .unwrap();
        let node = filesystem
            .tx(|tx| tx.find_node(TreePtr::<Node>::root(), dirname))
            .unwrap();
        assert_eq!(
            node.data().mode() & Node::MODE_PERM,
            0o0755 & Node::MODE_PERM
        );
    }

    #[test]
    fn default_dir_node_owners() {
        let mut filesystem = create_mock_filesystem();
        let dirname = "root";
        let dirpath = format!("/{dirname}");
        FileConfig::new_directory(dirpath)
            .create(&mut filesystem)
            .unwrap();
        let node = filesystem
            .tx(|tx| tx.find_node(TreePtr::<Node>::root(), dirname))
            .unwrap();
        assert_eq!(node.data().uid(), !0);
        assert_eq!(node.data().gid(), !0);
    }

    #[test]
    fn specify_file_node_mode_and_owners() {
        let mut filesystem = create_mock_filesystem();
        let filename = "foo.txt";
        let filepath = format!("/{filename}");
        let mode = 0o0123;
        let uid = 1234;
        let gid = 5678;
        FileConfig::new_file(filepath, "")
            .with_mod(mode, uid, gid)
            .create(&mut filesystem)
            .unwrap();
        let node = filesystem
            .tx(|tx| tx.find_node(TreePtr::<Node>::root(), filename))
            .unwrap();
        assert_eq!(
            node.data().mode() & Node::MODE_PERM,
            mode as u16 & Node::MODE_PERM
        );
        assert_eq!(node.data().uid(), uid);
        assert_eq!(node.data().gid(), gid);
    }

    #[test]
    fn specify_dir_node_mode_and_owners() {
        let mut filesystem = create_mock_filesystem();
        let dirname = "root";
        let subdirname = "subdir";
        let subdirpath = format!("/{dirname}/{subdirname}");
        let mode = 0o0123;
        let uid = 1234;
        let gid = 5678;
        FileConfig::new_directory(subdirpath)
            .with_mod(mode, uid, gid)
            .create(&mut filesystem)
            .unwrap();
        let dir_node = filesystem
            .tx(|tx| tx.find_node(TreePtr::<Node>::root(), dirname))
            .unwrap();
        assert_eq!(
            dir_node.data().mode() & Node::MODE_PERM,
            0o0755 & Node::MODE_PERM
        );
        let subdir_node = filesystem
            .tx(|tx| tx.find_node(TreePtr::<Node>::new(dir_node.id()), subdirname))
            .unwrap();
        assert_eq!(
            subdir_node.data().mode() & Node::MODE_PERM,
            mode as u16 & Node::MODE_PERM
        );
        assert_eq!(subdir_node.data().uid(), uid);
        assert_eq!(subdir_node.data().gid(), gid);
    }

    #[test]
    fn recursive_chown() {
        let mut filesystem = create_mock_filesystem();
        let recursive_chown_dirname = "foo";
        let recursive_chown_subdirname = "bar";
        let recursive_chown_dir_filename = "a.txt";
        let recursive_chown_subdir_filename = "b.txt";
        let recursive_chown_dirpath = format!("/{recursive_chown_dirname}");
        let recursive_chown_subdirpath =
            format!("/{recursive_chown_dirname}/{recursive_chown_subdirname}");
        let recursive_chown_dir_filepath =
            format!("/{recursive_chown_dirpath}/{recursive_chown_dir_filename}");
        let recursive_chown_subdir_filepath =
            format!("/{recursive_chown_subdirpath}/{recursive_chown_subdir_filename}");
        let adjacent_dirname = "root";
        let adjacent_dir_filename = "c.txt";
        let adjacent_subdirname = "stuff";
        let adjacent_dirpath = format!("/{adjacent_dirname}");
        let adjacent_dir_filepath = format!("/{adjacent_dirpath}/{adjacent_dir_filename}");
        let adjacent_subdirpath = format!("/{adjacent_dirpath}/{adjacent_subdirname}");

        // Create all dirs and files
        FileConfig::new_directory(recursive_chown_subdirpath)
            .create(&mut filesystem)
            .unwrap();
        FileConfig::new_file(recursive_chown_dir_filepath, "")
            .create(&mut filesystem)
            .unwrap();
        FileConfig::new_file(recursive_chown_subdir_filepath, "")
            .create(&mut filesystem)
            .unwrap();
        FileConfig::new_directory(adjacent_subdirpath)
            .create(&mut filesystem)
            .unwrap();
        FileConfig::new_file(adjacent_dir_filepath, "")
            .create(&mut filesystem)
            .unwrap();

        // Apply recursive chown on `/foo` by trying to create the dir once more, but with the
        // `with_recursive_mod()` method now used to activate the recursive chown for all its
        // contents
        let recursive_mode = 0o0123;
        let recursive_uid = 1234;
        let recursive_gid = 5678;
        FileConfig::new_directory(recursive_chown_dirpath)
            .with_recursive_mod(recursive_mode, recursive_uid, recursive_gid)
            .create(&mut filesystem)
            .unwrap();

        // Check `/foo` shows the effects of the recursive chown
        let recursive_chown_dir_node = filesystem
            .tx(|tx| tx.find_node(TreePtr::<Node>::root(), recursive_chown_dirname))
            .unwrap();
        assert_eq!(
            recursive_chown_dir_node.data().mode() & Node::MODE_PERM,
            recursive_mode as u16 & Node::MODE_PERM
        );
        assert!(recursive_chown_dir_node.data().is_dir());
        assert_eq!(recursive_chown_dir_node.data().uid(), recursive_uid);
        assert_eq!(recursive_chown_dir_node.data().gid(), recursive_gid);

        // Check `/foo/a.txt` shows the effects of the recursive chown
        let recursive_chown_dir_file_node = filesystem
            .tx(|tx| {
                tx.find_node(
                    TreePtr::<Node>::new(recursive_chown_dir_node.id()),
                    recursive_chown_dir_filename,
                )
            })
            .unwrap();
        assert_eq!(
            recursive_chown_dir_file_node.data().mode() & Node::MODE_PERM,
            recursive_mode as u16 & Node::MODE_PERM
        );
        assert_eq!(recursive_chown_dir_file_node.data().uid(), recursive_uid);
        assert_eq!(recursive_chown_dir_file_node.data().gid(), recursive_gid);

        // Check `/foo/bar` shows the effects of the recursive chown
        let recursive_chown_subdir_node = filesystem
            .tx(|tx| {
                tx.find_node(
                    TreePtr::<Node>::new(recursive_chown_dir_node.id()),
                    recursive_chown_subdirname,
                )
            })
            .unwrap();
        assert_eq!(
            recursive_chown_subdir_node.data().mode() & Node::MODE_PERM,
            recursive_mode as u16 & Node::MODE_PERM
        );
        assert_eq!(recursive_chown_subdir_node.data().uid(), recursive_uid);
        assert_eq!(recursive_chown_subdir_node.data().gid(), recursive_gid);

        // Check `/foo/bar/b.txt` shows the effects of the recursive chown
        let recursive_chown_subdir_file_node = filesystem
            .tx(|tx| {
                tx.find_node(
                    TreePtr::<Node>::new(recursive_chown_subdir_node.id()),
                    recursive_chown_subdir_filename,
                )
            })
            .unwrap();
        assert_eq!(
            recursive_chown_subdir_file_node.data().mode() & Node::MODE_PERM,
            recursive_mode as u16 & Node::MODE_PERM
        );
        assert_eq!(recursive_chown_subdir_file_node.data().uid(), recursive_uid);
        assert_eq!(recursive_chown_subdir_file_node.data().gid(), recursive_gid);

        // Check `/root` is unaffected by the recursive chown
        let adjacent_dir_node = filesystem
            .tx(|tx| tx.find_node(TreePtr::<Node>::root(), adjacent_dirname))
            .unwrap();
        assert_eq!(
            adjacent_dir_node.data().mode() & Node::MODE_PERM,
            0o0755 & Node::MODE_PERM
        );
        assert_eq!(adjacent_dir_node.data().uid(), 0);
        assert_eq!(adjacent_dir_node.data().gid(), 0);

        // Check `/root/c.txt` is unaffected by the recursive chown
        let adjacent_dir_file_node = filesystem
            .tx(|tx| {
                tx.find_node(
                    TreePtr::<Node>::new(adjacent_dir_node.id()),
                    adjacent_dir_filename,
                )
            })
            .unwrap();
        assert_eq!(
            adjacent_dir_file_node.data().mode() & Node::MODE_PERM,
            0o0644 & Node::MODE_PERM
        );
        assert_eq!(adjacent_dir_file_node.data().uid(), !0);
        assert_eq!(adjacent_dir_file_node.data().gid(), !0);

        // Check `/root/stuff` is unaffected by the recursive chown
        let adjacent_subdir_node = filesystem
            .tx(|tx| {
                tx.find_node(
                    TreePtr::<Node>::new(adjacent_dir_node.id()),
                    adjacent_subdirname,
                )
            })
            .unwrap();
        assert_eq!(
            adjacent_subdir_node.data().mode() & Node::MODE_PERM,
            0o0755 & Node::MODE_PERM
        );
        assert_eq!(adjacent_subdir_node.data().uid(), !0);
        assert_eq!(adjacent_subdir_node.data().gid(), !0);
    }
}
