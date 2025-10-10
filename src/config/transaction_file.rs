use redoxfs::{Disk, FileSystem, Node, TreePtr};

use crate::Result;

#[derive(Clone, Debug)]
pub struct FileConfig {
    path: String,
    data: String,
    directory: bool,
}

impl FileConfig {
    pub fn new_file(path: impl Into<String>, data: impl Into<String>) -> FileConfig {
        FileConfig {
            path: path.into(),
            data: data.into(),
            directory: false,
        }
    }

    pub fn new_directory(path: impl Into<String>) -> FileConfig {
        FileConfig {
            path: path.into(),
            data: String::new(),
            directory: true,
        }
    }

    pub(crate) fn create<D: Disk>(&self, filesystem: &mut FileSystem<D>) -> Result<()> {
        let filename = self.path.trim_start_matches("/");
        let ctime = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?;
        let mode = if self.directory {
            Node::MODE_DIR
        } else {
            Node::MODE_FILE
        };
        let node = filesystem.tx(|tx| {
            tx.create_node(
                TreePtr::<Node>::root(),
                filename,
                mode,
                ctime.as_secs(),
                ctime.subsec_nanos(),
            )
        })?;

        if !self.directory {
            filesystem.tx(|tx| {
                tx.write_node(
                    TreePtr::<Node>::new(node.id()),
                    0,
                    self.data.as_bytes(),
                    ctime.as_secs(),
                    ctime.subsec_nanos(),
                )
            })?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod test {
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
    fn write_file_node_in_root_dir() {
        let mut filesystem = create_mock_filesystem();
        let filename = "foo.txt";
        let path = format!("/{filename}");
        let data = "Hello, world!";
        FileConfig::new_file(path, data)
            .create(&mut filesystem)
            .unwrap();
        let node = filesystem
            .tx(|tx| tx.find_node(TreePtr::<Node>::root(), filename))
            .unwrap();
        let mut buf = [0; 13];
        filesystem
            .tx(|tx| tx.read_node(TreePtr::<Node>::new(node.id()), 0, &mut buf, 1, 0))
            .unwrap();
        assert!(node.data().is_file());
        assert_eq!(&buf, data.as_bytes());
    }

    #[test]
    fn write_dir_node_in_root_dir() {
        let mut filesystem = create_mock_filesystem();
        let dirname = "root";
        let path = format!("/{dirname}");
        FileConfig::new_directory(path)
            .create(&mut filesystem)
            .unwrap();
        let node = filesystem
            .tx(|tx| tx.find_node(TreePtr::<Node>::root(), dirname))
            .unwrap();
        assert!(node.data().is_dir());
    }
}
