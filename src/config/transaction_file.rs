use std::{
    ffi::OsStr,
    path::{Component, Path},
    time::Duration,
};

use redoxfs::{Disk, FileSystem, Node, TreePtr};

use crate::Result;

#[derive(Clone, Debug, Default)]
pub struct FileConfig {
    path: String,
    data: String,
    symlink: bool,
    directory: bool,
    mode: Option<u32>,
    uid: Option<u32>,
    gid: Option<u32>,
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
        let mut iter = Path::new(&self.path).components();
        let filename = if let Component::Normal(val) = iter
            .next_back()
            .expect("Expected at least one element in path-components iterator")
        {
            val
        } else {
            panic!("Expected final path-component of non-directory FileConfig to be a filename");
        };
        let mut parent_id = TreePtr::<Node>::root().id();

        for dir in iter {
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
        let mut parent_id = TreePtr::<Node>::root().id();
        let mut iter = Path::new(&self.path).components().peekable();

        while let Some(dir) = iter.next() {
            let parent_ptr = TreePtr::<Node>::new(parent_id);
            match dir {
                Component::RootDir => continue,
                Component::Normal(subdir) => {
                    let node = filesystem.tx(|tx| {
                        tx.create_node(
                            parent_ptr,
                            subdir.to_str().expect(&format!(
                                "Expected subdir name to be valid utf-8: {:?}",
                                subdir
                            )),
                            Node::MODE_DIR | self.mode.unwrap_or(0o0755) as u16 & Node::MODE_PERM,
                            ctime.as_secs(),
                            ctime.subsec_nanos(),
                        )
                    })?;
                    if iter.peek().is_none() {
                        continue;
                    }
                    parent_id = node.id();
                }
                _ => todo!(),
            }
        }

        let dirname = if let Component::Normal(dir) = Path::new(&self.path)
            .components()
            .next_back()
            .expect("Safe as iterator has length greater than 1")
        {
            dir
        } else {
            OsStr::new("/")
        };
        self.apply_owners(filesystem, parent_id, dirname)?;

        Ok(())
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
        filesystem.tx(|tx| tx.sync_tree(node))?;
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
        let dirpath = format!("/{dirname}");
        let mode = 0o0123;
        let uid = 1234;
        let gid = 5678;
        FileConfig::new_directory(dirpath)
            .with_mod(mode, uid, gid)
            .create(&mut filesystem)
            .unwrap();
        let node = filesystem
            .tx(|tx| tx.find_node(TreePtr::<Node>::root(), dirname))
            .unwrap();
        assert_eq!(
            node.data().mode() & Node::MODE_PERM,
            mode as u16 & Node::MODE_PERM
        );
        assert_eq!(node.data().uid(), uid);
        assert_eq!(node.data().gid(), gid);
    }
}
