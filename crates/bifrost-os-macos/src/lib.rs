//! Portable services for a standalone x86-64 Mach-O guest.
use bifrost_core::{GuestAddress, GuestMemoryRead, X86State};
use std::{fmt, io::Write};

pub struct Io<'a> {
    pub stdout: &'a mut dyn Write,
    pub stderr: &'a mut dyn Write,
}

#[derive(Debug)]
pub struct UnsupportedSyscall {
    pub number: u64,
    pub pc: GuestAddress,
}
impl fmt::Display for UnsupportedSyscall {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unsupported Darwin x86-64 syscall {:#x} at {:#x}",
            self.number, self.pc.0
        )
    }
}
impl std::error::Error for UnsupportedSyscall {}

pub enum Action {
    Continue,
    Exit(u8),
}

impl Io<'_> {
    pub fn syscall(
        &mut self,
        cpu: &mut X86State,
        memory: &dyn GuestMemoryRead,
    ) -> Result<Action, UnsupportedSyscall> {
        let result = match cpu.gpr[0] {
            0x0200_0001 => return Ok(Action::Exit(cpu.gpr[7] as u8)),
            0x0200_0004 => self.write(cpu.gpr[7], GuestAddress(cpu.gpr[6]), cpu.gpr[2], memory),
            number => {
                return Err(UnsupportedSyscall {
                    number,
                    pc: cpu.rip,
                });
            }
        };
        let next = cpu.rip.0.wrapping_add(2);
        cpu.gpr[1] = next;
        cpu.gpr[11] = cpu.rflags;
        cpu.rflags &= !1;
        cpu.gpr[0] = match result {
            Ok(count) => count as u64,
            Err(errno) => {
                cpu.rflags |= 1;
                errno
            }
        };
        cpu.rip = GuestAddress(next);
        Ok(Action::Continue)
    }

    fn write(
        &mut self,
        fd: u64,
        address: GuestAddress,
        count: u64,
        memory: &dyn GuestMemoryRead,
    ) -> Result<usize, u64> {
        let out = match fd {
            1 => &mut self.stdout,
            2 => &mut self.stderr,
            _ => return Err(9), // EBADF
        };
        let mut bytes = vec![0; count.min(65536) as usize];
        if !bytes.is_empty()
            && (!address.is_canonical_48() || memory.read(address, &mut bytes).is_err())
        {
            return Err(14); // EFAULT
        }
        out.write(&bytes).map_err(|_| 5) // EIO
    }
}
