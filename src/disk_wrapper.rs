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

enum Buffer<'a> {
    Read(&'a mut [u8]),
    Write(&'a [u8]),
}

impl DiskWrapper {
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let disk = OpenOptions::new().read(true).write(true).open(path)?;
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

    fn io<'a>(&mut self, buf: &mut Buffer<'a>) -> Result<usize> {
        let buf_len = match buf {
            Buffer::Read(read) => read.len(),
            Buffer::Write(write) => write.len(),
        };
        let block_len: u64 = self.block.len().try_into().unwrap();

        // Do aligned I/O quickly
        if self.seek % block_len == 0 && buf_len as u64 % block_len == 0 {
            self.disk.seek(SeekFrom::Start(self.seek))?;
            match buf {
                Buffer::Read(read) => self.disk.read_exact(read)?,
                Buffer::Write(write) => self.disk.write_all(write)?,
            }
            self.seek = self.seek.checked_add(buf_len.try_into().unwrap()).unwrap();
            return Ok(buf_len);
        }

        let mut i = 0;
        while i < buf_len {
            let block = self.seek / block_len;
            let offset: usize = (self.seek % block_len).try_into().unwrap();
            let remaining = buf_len.checked_sub(i).unwrap();
            let len = cmp::min(
                remaining,
                self.block
                    .len()
                    .checked_sub(offset.try_into().unwrap())
                    .unwrap(),
            );

            self.disk
                .seek(SeekFrom::Start(block.checked_mul(block_len).unwrap()))?;
            self.disk.read_exact(&mut self.block)?;

            match buf {
                Buffer::Read(read) => {
                    read[i..i.checked_add(len).unwrap()]
                        .copy_from_slice(&self.block[offset..offset.checked_add(len).unwrap()]);
                }
                Buffer::Write(write) => {
                    self.block[offset..offset.checked_add(len).unwrap()]
                        .copy_from_slice(&write[i..i.checked_add(len).unwrap()]);

                    self.disk
                        .seek(SeekFrom::Start(block.checked_mul(block_len).unwrap()))?;
                    self.disk.write_all(&mut self.block)?;
                }
            }

            i = i.checked_add(len).unwrap();
            self.seek = self.seek.checked_add(len.try_into().unwrap()).unwrap();
        }

        Ok(i)
    }
}

impl Read for DiskWrapper {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        self.io(&mut Buffer::Read(buf))
    }
}

impl Seek for DiskWrapper {
    fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
        let current: i64 = self.seek.try_into().unwrap();
        let end: i64 = self.size.try_into().unwrap();
        self.seek = match pos {
            SeekFrom::Start(offset) => cmp::min(self.size, offset),
            SeekFrom::End(offset) => cmp::max(0, cmp::min(end, end.wrapping_add(offset))) as u64,
            SeekFrom::Current(offset) => {
                cmp::max(0, cmp::min(end, current.wrapping_add(offset))) as u64
            }
        };
        Ok(self.seek)
    }
}

impl Write for DiskWrapper {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        self.io(&mut Buffer::Write(buf))
    }

    fn flush(&mut self) -> Result<()> {
        self.disk.flush()
    }
}
