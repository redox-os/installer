use std::{
    cmp,
    convert::TryInto,
    fs::{File, OpenOptions},
    io::{Read, Result, Seek, SeekFrom, Write},
    path::Path,
};

#[derive(Debug)]
pub struct DiskWrapper {
    disk: File,
    size: u64,
    block: Box<[u8]>,
    seek: u64,
}

impl DiskWrapper {
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let disk = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)?;
        let metadata = disk.metadata()?;
        let size = metadata.len();
        // TODO: get real block size: disk_metadata.blksize() works on disks but not image files
        let block_size = 512;
        let block = vec![0u8; block_size].into_boxed_slice();
        Ok(Self {
            disk,
            size,
            block,
            seek: 0,
        })
    }

    pub fn block_size(&self) -> usize {
        self.block.len()
    }

    pub fn size(&self) -> u64 {
        self.size
    }
}

impl Read for DiskWrapper {
    //TODO: improve performance by directly using block aligned parts of buf
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let mut i = 0;
        while i < buf.len() {
            let block_len: u64 = self.block.len().try_into().unwrap();
            let block = self.seek / block_len;
            let offset: usize = (self.seek % block_len).try_into().unwrap();
            let remaining = buf.len().checked_sub(i).unwrap();
            let len = cmp::min(
                remaining,
                self.block.len().checked_sub(offset.try_into().unwrap()).unwrap()
            );

            self.disk.seek(SeekFrom::Start(block.checked_mul(block_len).unwrap()))?;
            self.disk.read_exact(&mut self.block)?;

            buf[i..i.checked_add(len).unwrap()].copy_from_slice(
                &self.block[offset..offset.checked_add(len).unwrap()]
            );

            i = i.checked_add(len).unwrap();
            self.seek = self.seek.checked_add(len.try_into().unwrap()).unwrap();
        }
        Ok(i)
    }
}

impl Seek for DiskWrapper {
    fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
        let current: i64 = self.seek.try_into().unwrap();
        let end: i64 = self.size.try_into().unwrap();
        self.seek = match pos {
            SeekFrom::Start(offset) => {
                cmp::min(self.size, offset)
            },
            SeekFrom::End(offset) => {
                cmp::max(0, cmp::min(end, end.wrapping_add(offset))) as u64
            },
            SeekFrom::Current(offset) => {
                cmp::max(0, cmp::min(end, current.wrapping_add(offset))) as u64
            }
        };
        Ok(self.seek)
    }
}

impl Write for DiskWrapper {
    //TODO: improve performance by directly using block aligned parts of buf
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        let mut i = 0;
        while i < buf.len() {
            let block_len: u64 = self.block.len().try_into().unwrap();
            let block = self.seek / block_len;
            let offset: usize = (self.seek % block_len).try_into().unwrap();
            let remaining = buf.len().checked_sub(i).unwrap();
            let len = cmp::min(
                remaining,
                self.block.len().checked_sub(offset.try_into().unwrap()).unwrap()
            );

            self.disk.seek(SeekFrom::Start(block.checked_mul(block_len).unwrap()))?;
            self.disk.read_exact(&mut self.block)?;

            self.block[offset..offset.checked_add(len).unwrap()].copy_from_slice(
                &buf[i..i.checked_add(len).unwrap()]
            );

            self.disk.seek(SeekFrom::Start(block.checked_mul(block_len).unwrap()))?;
            self.disk.write_all(&mut self.block)?;

            i = i.checked_add(len).unwrap();
            self.seek = self.seek.checked_add(len.try_into().unwrap()).unwrap();
        }
        Ok(i)
    }

    fn flush(&mut self) -> Result<()> {
        self.disk.flush()
    }
}
