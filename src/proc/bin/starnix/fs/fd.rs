// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bitflags::bitflags;
use fuchsia_zircon as zx;
use parking_lot::{Mutex, RwLock};
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use zerocopy::{AsBytes, FromBytes};

use crate::fs::*;
use crate::not_implemented;
use crate::syscalls::SyscallResult;
use crate::task::*;
use crate::types::*;

pub use starnix_macros::FileObject;

#[derive(Hash, PartialEq, Eq, Debug, Copy, Clone, AsBytes, FromBytes)]
#[repr(transparent)]
pub struct FdNumber(i32);

impl FdNumber {
    pub const AT_FDCWD: FdNumber = FdNumber(AT_FDCWD);

    pub fn from_raw(n: i32) -> FdNumber {
        FdNumber(n)
    }
    pub fn raw(&self) -> i32 {
        self.0
    }
}

impl fmt::Display for FdNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "fd({})", self.0)
    }
}

pub enum SeekOrigin {
    SET,
    CUR,
    END,
}

impl SeekOrigin {
    pub fn from_raw(whence: u32) -> Result<SeekOrigin, Errno> {
        match whence {
            SEEK_SET => Ok(SeekOrigin::SET),
            SEEK_CUR => Ok(SeekOrigin::CUR),
            SEEK_END => Ok(SeekOrigin::END),
            _ => Err(EINVAL),
        }
    }
}

/// Corresponds to struct file_operations in Linux, plus any filesystem-specific data.
pub trait FileOps: Send + Sync {
    /// Read from the file without an offset. If your file is seekable, consider implementing this
    /// with fd_impl_seekable.
    fn read(&self, file: &FileObject, task: &Task, data: &[UserBuffer]) -> Result<usize, Errno>;
    /// Read from the file at an offset. If your file is seekable, consider implementing this with
    /// fd_impl_nonseekable!.
    fn read_at(
        &self,
        file: &FileObject,
        task: &Task,
        offset: usize,
        data: &[UserBuffer],
    ) -> Result<usize, Errno>;
    /// Write to the file without an offset. If your file is seekable, consider implementing this
    /// with fd_impl_seekable!.
    fn write(&self, file: &FileObject, task: &Task, data: &[UserBuffer]) -> Result<usize, Errno>;
    /// Write to the file at a offset. If your file is nonseekable, consider implementing this with
    /// fd_impl_nonseekable!.
    fn write_at(
        &self,
        file: &FileObject,
        task: &Task,
        offset: usize,
        data: &[UserBuffer],
    ) -> Result<usize, Errno>;

    /// Adjust the seek offset if the file is seekable.
    fn seek(
        &self,
        file: &FileObject,
        task: &Task,
        offset: off_t,
        whence: SeekOrigin,
    ) -> Result<off_t, Errno>;

    /// Responds to an mmap call by returning a VMO. At least the requested protection flags must
    /// be set on the VMO. Reading or writing the VMO must read or write the file. If this is not
    /// possible given the requested protection, an error must be returned.
    fn get_vmo(
        &self,
        _file: &FileObject,
        _task: &Task,
        _prot: zx::VmarFlags,
    ) -> Result<zx::Vmo, Errno> {
        Err(ENODEV)
    }

    fn ioctl(
        &self,
        _file: &FileObject,
        _task: &Task,
        request: u32,
        _in_addr: UserAddress,
        _out_addr: UserAddress,
    ) -> Result<SyscallResult, Errno> {
        default_ioctl(request)
    }

    fn fcntl(
        &self,
        _file: &FileObject,
        _task: &Task,
        _cmd: u32,
        _arg: u64,
    ) -> Result<SyscallResult, Errno> {
        Err(EINVAL)
    }
}

/// Implements FileDesc methods in a way that makes sense for nonseekable files. You must implement
/// read and write.
#[macro_export]
macro_rules! fd_impl_nonseekable {
    () => {
        fn read_at(
            &self,
            _file: &FileObject,
            _task: &Task,
            _offset: usize,
            _data: &[UserBuffer],
        ) -> Result<usize, Errno> {
            Err(ESPIPE)
        }
        fn write_at(
            &self,
            _file: &FileObject,
            _task: &Task,
            _offset: usize,
            _data: &[UserBuffer],
        ) -> Result<usize, Errno> {
            Err(ESPIPE)
        }
        fn seek(
            &self,
            _file: &FileObject,
            _task: &Task,
            _offset: off_t,
            _whence: SeekOrigin,
        ) -> Result<off_t, Errno> {
            Err(ESPIPE)
        }
    };
}

/// Implements FileDesc methods in a way that makes sense for seekable files. You must implement
/// read_at and write_at.
#[macro_export]
macro_rules! fd_impl_seekable {
    () => {
        fn read(
            &self,
            file: &FileObject,
            task: &Task,
            data: &[UserBuffer],
        ) -> Result<usize, Errno> {
            let mut offset = file.offset.lock();
            let size = self.read_at(file, task, *offset, data)?;
            *offset += size;
            Ok(size)
        }
        fn write(
            &self,
            file: &FileObject,
            task: &Task,
            data: &[UserBuffer],
        ) -> Result<usize, Errno> {
            let mut offset = file.offset.lock();
            let size = self.write_at(file, task, *offset, data)?;
            *offset += size;
            Ok(size)
        }
        fn seek(
            &self,
            _file: &FileObject,
            _task: &Task,
            _offset: off_t,
            _whence: SeekOrigin,
        ) -> Result<off_t, Errno> {
            not_implemented!("seek is not implemented");
            Err(ENOSYS)
        }
    };
}

pub fn default_ioctl(request: u32) -> Result<SyscallResult, Errno> {
    not_implemented!("ioctl: request=0x{:x}", request);
    Err(ENOTTY)
}

/// Corresponds to struct file in Linux.
pub struct FileObject {
    ops: Box<dyn FileOps>,
    pub node: FsNodeHandle,
    pub offset: Mutex<usize>,
    pub flags: Mutex<u32>,
    pub async_owner: Mutex<pid_t>,
}

impl FileObject {
    pub fn new<T: FileOps + 'static>(ops: T, node: FsNodeHandle) -> FileHandle {
        Self::new_from_box(Box::new(ops), node)
    }
    pub fn new_from_box(ops: Box<dyn FileOps>, node: FsNodeHandle) -> FileHandle {
        Arc::new(Self {
            node,
            ops,
            offset: Mutex::new(0),
            flags: Mutex::new(0),
            async_owner: Mutex::new(0),
        })
    }

    fn ops(&self) -> &dyn FileOps {
        &*self.ops
    }

    fn blocking_op<T, Op>(&self, task: &Task, op: Op, events: FdEvents) -> Result<T, Errno>
    where
        Op: Fn() -> Result<T, Errno>,
    {
        loop {
            match op() {
                Err(EAGAIN) if !self.has_file_flag(O_NONBLOCK) => {
                    self.node.observers.wait_once(&task.waiter, events)?;
                }
                result => return result,
            }
        }
    }

    pub fn read(&self, task: &Task, data: &[UserBuffer]) -> Result<usize, Errno> {
        self.blocking_op(task, || self.ops().read(self, task, data), FdEvents::POLLIN)
    }

    pub fn read_at(&self, task: &Task, offset: usize, data: &[UserBuffer]) -> Result<usize, Errno> {
        self.blocking_op(task, || self.ops().read_at(self, task, offset, data), FdEvents::POLLIN)
    }

    pub fn write(&self, task: &Task, data: &[UserBuffer]) -> Result<usize, Errno> {
        self.blocking_op(task, || self.ops().write(self, task, data), FdEvents::POLLOUT)
    }

    pub fn write_at(
        &self,
        task: &Task,
        offset: usize,
        data: &[UserBuffer],
    ) -> Result<usize, Errno> {
        self.blocking_op(task, || self.ops().write_at(self, task, offset, data), FdEvents::POLLOUT)
    }

    pub fn seek(&self, task: &Task, offset: off_t, whence: SeekOrigin) -> Result<off_t, Errno> {
        self.ops().seek(self, task, offset, whence)
    }

    pub fn get_vmo(&self, task: &Task, prot: zx::VmarFlags) -> Result<zx::Vmo, Errno> {
        self.ops().get_vmo(self, task, prot)
    }

    pub fn ioctl(
        &self,
        task: &Task,
        request: u32,
        in_addr: UserAddress,
        out_addr: UserAddress,
    ) -> Result<SyscallResult, Errno> {
        self.ops().ioctl(self, task, request, in_addr, out_addr)
    }

    pub fn fcntl(&self, task: &Task, cmd: u32, arg: u64) -> Result<SyscallResult, Errno> {
        self.ops().fcntl(self, task, cmd, arg)
    }

    pub fn set_file_flags(&self, value: u32) {
        let mut flags = self.flags.lock();
        *flags = value;
    }

    pub fn has_file_flag(&self, flag: u32) -> bool {
        let flags = self.flags.lock();
        return *flags & flag != 0;
    }

    /// Get the async owner of this file.
    ///
    /// See fcntl(F_GETOWN)
    pub fn get_async_owner(&self) -> pid_t {
        *self.async_owner.lock()
    }

    /// Set the async owner of this file.
    ///
    /// See fcntl(F_SETOWN)
    pub fn set_async_owner(&self, owner: pid_t) {
        *self.async_owner.lock() = owner;
    }
}

pub type FileHandle = Arc<FileObject>;

bitflags! {
    pub struct FdFlags: u32 {
        const CLOEXEC = FD_CLOEXEC;
    }
}

#[derive(Clone)]
struct FdTableEntry {
    file: FileHandle,

    // Rather than using a separate "flags" field, we could maintain this data
    // as a bitfield over the file descriptors because there is only one flag
    // currently (CLOEXEC) and file descriptor numbers tend to cluster near 0.
    flags: FdFlags,
}

impl FdTableEntry {
    fn new(file: FileHandle, flags: FdFlags) -> FdTableEntry {
        FdTableEntry { file, flags }
    }
}

pub struct FdTable {
    table: RwLock<HashMap<FdNumber, FdTableEntry>>,
}

impl FdTable {
    pub fn new() -> Arc<FdTable> {
        Arc::new(FdTable { table: RwLock::new(HashMap::new()) })
    }

    pub fn fork(&self) -> Arc<FdTable> {
        Arc::new(FdTable { table: RwLock::new(self.table.read().clone()) })
    }

    pub fn exec(&self) {
        let mut table = self.table.write();
        table.retain(|_fd, entry| !entry.flags.contains(FdFlags::CLOEXEC));
    }

    pub fn insert(&self, fd: FdNumber, file: FileHandle) {
        let mut table = self.table.write();
        table.insert(fd, FdTableEntry::new(file, FdFlags::empty()));
    }

    pub fn add(&self, file: FileHandle) -> Result<FdNumber, Errno> {
        self.add_with_flags(file, FdFlags::empty())
    }

    pub fn add_with_flags(&self, file: FileHandle, flags: FdFlags) -> Result<FdNumber, Errno> {
        let mut table = self.table.write();
        let mut fd = FdNumber::from_raw(0);
        while table.contains_key(&fd) {
            fd = FdNumber::from_raw(fd.raw() + 1);
        }
        table.insert(fd, FdTableEntry::new(file, flags));
        Ok(fd)
    }

    pub fn get(&self, fd: FdNumber) -> Result<FileHandle, Errno> {
        let table = self.table.read();
        table.get(&fd).map(|entry| entry.file.clone()).ok_or(EBADF)
    }

    pub fn close(&self, fd: FdNumber) -> Result<(), Errno> {
        let mut table = self.table.write();
        table.remove(&fd).ok_or(EBADF).map(|_| {})
    }

    pub fn get_fd_flags(&self, fd: FdNumber) -> Result<FdFlags, Errno> {
        let table = self.table.read();
        table.get(&fd).map(|entry| entry.flags).ok_or(EBADF)
    }

    pub fn set_fd_flags(&self, fd: FdNumber, flags: FdFlags) -> Result<(), Errno> {
        let mut table = self.table.write();
        table
            .get_mut(&fd)
            .map(|entry| {
                entry.flags = flags;
            })
            .ok_or(EBADF)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::fs::SyslogFile;

    #[test]
    fn test_fd_table_install() {
        let kern = Kernel::new_for_testing();
        let files = FdTable::new();
        let file = SyslogFile::new(&kern);

        let fd0 = files.add(file.clone()).unwrap();
        assert_eq!(fd0.raw(), 0);
        let fd1 = files.add(file.clone()).unwrap();
        assert_eq!(fd1.raw(), 1);

        assert!(Arc::ptr_eq(&files.get(fd0).unwrap(), &file));
        assert!(Arc::ptr_eq(&files.get(fd1).unwrap(), &file));
        assert_eq!(files.get(FdNumber::from_raw(fd1.raw() + 1)).map(|_| ()), Err(EBADF));
    }

    #[test]
    fn test_fd_table_fork() {
        let kern = Kernel::new_for_testing();
        let files = FdTable::new();
        let file = SyslogFile::new(&kern);

        let fd0 = files.add(file.clone()).unwrap();
        let fd1 = files.add(file.clone()).unwrap();
        let fd2 = FdNumber::from_raw(2);

        let forked = files.fork();

        assert_eq!(Arc::as_ptr(&files.get(fd0).unwrap()), Arc::as_ptr(&forked.get(fd0).unwrap()));
        assert_eq!(Arc::as_ptr(&files.get(fd1).unwrap()), Arc::as_ptr(&forked.get(fd1).unwrap()));
        assert!(files.get(fd2).is_err());
        assert!(forked.get(fd2).is_err());

        files.set_fd_flags(fd0, FdFlags::CLOEXEC).unwrap();
        assert_eq!(FdFlags::CLOEXEC, files.get_fd_flags(fd0).unwrap());
        assert_ne!(FdFlags::CLOEXEC, forked.get_fd_flags(fd0).unwrap());
    }

    #[test]
    fn test_fd_table_exec() {
        let kern = Kernel::new_for_testing();
        let files = FdTable::new();
        let file = SyslogFile::new(&kern);

        let fd0 = files.add(file.clone()).unwrap();
        let fd1 = files.add(file.clone()).unwrap();

        files.set_fd_flags(fd0, FdFlags::CLOEXEC).unwrap();

        assert!(files.get(fd0).is_ok());
        assert!(files.get(fd1).is_ok());

        files.exec();

        assert!(!files.get(fd0).is_ok());
        assert!(files.get(fd1).is_ok());
    }

    #[test]
    fn test_fd_table_pack_values() {
        let kern = Kernel::new_for_testing();
        let files = FdTable::new();
        let file = SyslogFile::new(&kern);

        // Add two FDs.
        let fd0 = files.add(file.clone()).unwrap();
        let fd1 = files.add(file.clone()).unwrap();
        assert_eq!(fd0.raw(), 0);
        assert_eq!(fd1.raw(), 1);

        // Close FD 0
        assert!(files.close(fd0).is_ok());
        assert!(files.close(fd0).is_err());
        // Now it's gone.
        assert!(files.get(fd0).is_err());

        // The next FD we insert fills in the hole we created.
        let another_fd = files.add(file.clone()).unwrap();
        assert_eq!(another_fd.raw(), 0);
    }
}
