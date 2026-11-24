//! Bounded Linux x86-64 syscall provider with explicit standard streams.
use bifrost_core::{
    GuestAddress, GuestIsa, GuestMemory, GuestThreadId, X86State, validate_guest_access,
};
pub use bifrost_vfs::Vfs;
use std::{collections::BTreeMap, fmt, io::Write, num::NonZeroU8, sync::Arc};

const EFAULT: i64 = -14;
const EBADF: i64 = -9;
const EINVAL: i64 = -22;
const ENOENT: i64 = -2;
const EACCES: i64 = -13;
const EIO: i64 = -5;
const EMFILE: i64 = -24;
const ESPIPE: i64 = -29;
const EOVERFLOW: i64 = -75;
const MAX_IO: u64 = 65536;

pub struct Io<'a> {
    pub input: &'a [u8],
    pub stdout: &'a mut dyn Write,
    pub stderr: &'a mut dyn Write,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UnsupportedSyscall {
    pub number: u64,
    pub pc: GuestAddress,
}
impl fmt::Display for UnsupportedSyscall {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unsupported Linux x86-64 syscall {} at {:#x}",
            self.number, self.pc.0
        )
    }
}
impl std::error::Error for UnsupportedSyscall {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyscallExit {
    Continue,
    Exit(u8),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThreadSyscall {
    Continue,
    Yield,
    ExitThread(u8),
    ExitGroup(u8),
}
struct File {
    data: Arc<[u8]>,
    offset: i64,
}

pub struct Linux<'a> {
    io: Io<'a>,
    vfs: &'a Vfs,
    files: BTreeMap<u64, File>,
    input_offset: i64,
    closed_standard: [bool; 3],
}

impl<'a> Linux<'a> {
    pub fn new(io: Io<'a>, vfs: &'a Vfs) -> Self {
        Self {
            io,
            vfs,
            files: BTreeMap::new(),
            input_offset: 0,
            closed_standard: [false; 3],
        }
    }

    /// Handles an already fetched two-byte SYSCALL at the current guest PC.
    pub fn syscall(
        &mut self,
        cpu: &mut X86State,
        memory: &mut dyn GuestMemory,
    ) -> Result<SyscallExit, UnsupportedSyscall> {
        Ok(match self.thread_syscall(cpu, GuestThreadId(1), memory)? {
            ThreadSyscall::Continue | ThreadSyscall::Yield => SyscallExit::Continue,
            ThreadSyscall::ExitThread(status) | ThreadSyscall::ExitGroup(status) => {
                SyscallExit::Exit(status)
            }
        })
    }

    /// Handles a syscall for a thread in a cooperatively scheduled guest process.
    pub fn thread_syscall(
        &mut self,
        cpu: &mut X86State,
        thread: GuestThreadId,
        memory: &mut dyn GuestMemory,
    ) -> Result<ThreadSyscall, UnsupportedSyscall> {
        let [number, arg0, arg1, arg2] = [cpu.gpr[0], cpu.gpr[7], cpu.gpr[6], cpu.gpr[2]];
        let result = match number {
            0 => self.read(arg0, GuestAddress(arg1), arg2, memory),
            1 => self.write(arg0, GuestAddress(arg1), arg2, memory),
            3 => self.close(arg0),
            8 => self.seek(arg0, arg1 as i64, arg2),
            24 => 0, // sched_yield
            39 => 1, // Synthetic process ID.
            60 => return Ok(ThreadSyscall::ExitThread(arg0 as u8)),
            186 => thread.0 as i64,
            231 => return Ok(ThreadSyscall::ExitGroup(arg0 as u8)),
            257 => self.openat(arg0, GuestAddress(arg1), arg2, memory),
            _ => {
                return Err(UnsupportedSyscall {
                    number,
                    pc: cpu.rip,
                });
            }
        };
        let next = cpu.rip.0.wrapping_add(2);
        cpu.gpr[0] = result as u64;
        cpu.gpr[1] = next; // RCX
        cpu.gpr[11] = cpu.rflags;
        cpu.rip = GuestAddress(next);
        if number == 24 {
            Ok(ThreadSyscall::Yield)
        } else {
            Ok(ThreadSyscall::Continue)
        }
    }

    fn write(
        &mut self,
        fd: u64,
        address: GuestAddress,
        count: u64,
        memory: &dyn GuestMemory,
    ) -> i64 {
        let out = match fd {
            1 if !self.closed_standard[1] => &mut self.io.stdout,
            2 if !self.closed_standard[2] => &mut self.io.stderr,
            _ => return EBADF,
        };
        let mut bytes = vec![0; count.min(MAX_IO) as usize];
        if !valid_span(address, bytes.len()) || memory.read(address, &mut bytes).is_err() {
            return EFAULT;
        }
        out.write(&bytes).map_or(EIO, |count| count as i64)
    }

    fn read(
        &mut self,
        fd: u64,
        address: GuestAddress,
        count: u64,
        memory: &mut dyn GuestMemory,
    ) -> i64 {
        let (data, offset) = if fd == 0 && !self.closed_standard[0] {
            (self.io.input, &mut self.input_offset)
        } else if let Some(file) = self.files.get_mut(&fd) {
            (&*file.data, &mut file.offset)
        } else {
            return EBADF;
        };
        let start = usize::try_from(*offset)
            .unwrap_or(usize::MAX)
            .min(data.len());
        let len = (data.len() - start).min(count.min(MAX_IO) as usize);
        if !valid_span(address, len) || memory.write(address, &data[start..start + len]).is_err() {
            return EFAULT;
        }
        *offset += len as i64;
        len as i64
    }

    fn seek(&mut self, fd: u64, offset: i64, origin: u64) -> i64 {
        if fd < 3 {
            return if self.closed_standard[fd as usize] {
                EBADF
            } else {
                ESPIPE
            };
        }
        let Some(file) = self.files.get_mut(&fd) else {
            return EBADF;
        };
        let base = match origin {
            0 => 0,
            1 => file.offset,
            2 => file.data.len() as i64,
            _ => return EINVAL,
        };
        let Some(position) = base.checked_add(offset) else {
            return EOVERFLOW;
        };
        if position < 0 {
            return EINVAL;
        }
        file.offset = position;
        position
    }

    fn close(&mut self, fd: u64) -> i64 {
        if let Some(closed) = self.closed_standard.get_mut(fd as usize).filter(|_| fd < 3) {
            if *closed {
                return EBADF;
            }
            *closed = true;
            return 0;
        }
        if self.files.remove(&fd).is_some() {
            0
        } else {
            EBADF
        }
    }

    fn openat(
        &mut self,
        dir: u64,
        address: GuestAddress,
        flags: u64,
        memory: &dyn GuestMemory,
    ) -> i64 {
        if dir as i64 != -100 || flags & !0x80000 != 0 {
            return EINVAL;
        }
        let mut path = Vec::new();
        for i in 0..4096 {
            let mut byte = [0];
            let Some(address) = address.checked_add(i) else {
                return EFAULT;
            };
            if !address.is_canonical_48() || memory.read(address, &mut byte).is_err() {
                return EFAULT;
            }
            if byte[0] == 0 {
                let data = match self.vfs.open(&path) {
                    Ok(Some(data)) => data,
                    Ok(None) => return ENOENT,
                    Err(_) => return EACCES,
                };
                let Some(fd) = (3..1024).find(|id| !self.files.contains_key(id)) else {
                    return EMFILE;
                };
                self.files.insert(fd, File { data, offset: 0 });
                return fd as i64;
            }
            path.push(byte[0]);
        }
        EINVAL
    }
}

fn valid_span(address: GuestAddress, len: usize) -> bool {
    let Some(last) = address.checked_add(len.saturating_sub(1) as u64) else {
        return false;
    };
    len == 0
        || (validate_guest_access(GuestIsa::X86_64, address, NonZeroU8::MIN, NonZeroU8::MIN)
            .is_ok()
            && last.is_canonical_48()
            && ((last.0 ^ address.0) >> 47) == 0)
}
