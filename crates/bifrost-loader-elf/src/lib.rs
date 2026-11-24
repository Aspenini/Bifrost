//! Static ELF64 images for the initial Linux x86-64 process profile.
use bifrost_core::{GuestAddress, GuestIsa, GuestMemory, Protection, X86State};
use bifrost_loader::{
    Format, Image, LoadError, Metadata, PAGE, Segment, slice, u16_at, u32_at, u64_at,
};
use bifrost_memory::AddressSpace;

const STACK: u64 = 0x7fff_ffff_0000;
const STACK_SIZE: u64 = 64 * PAGE;

pub struct Elf<'a> {
    pub image: Image<'a>,
    phdr: Option<u64>,
    phnum: u16,
}

pub fn parse(bytes: &[u8]) -> Result<Elf<'_>, LoadError> {
    if slice(bytes, 0, 7)? != b"\x7fELF\x02\x01\x01" {
        return Err(LoadError::Unsupported("requires little-endian ELF64"));
    }
    if !matches!(slice(bytes, 7, 1)?[0], 0 | 3) {
        return Err(LoadError::Unsupported("ELF OS ABI"));
    }
    if u16_at(bytes, 16)? != 2 || u16_at(bytes, 18)? != 62 {
        return Err(LoadError::Unsupported("requires static x86-64 ET_EXEC"));
    }
    if u32_at(bytes, 20)? != 1 || u16_at(bytes, 52)? != 64 || u16_at(bytes, 54)? != 56 {
        return Err(LoadError::Invalid("ELF header sizes or version"));
    }
    let metadata = Metadata {
        format: Format::Elf,
        isa: GuestIsa::X86_64,
        entry: GuestAddress(u64_at(bytes, 24)?),
    };
    let phoff = u64_at(bytes, 32)?;
    let phnum = u16_at(bytes, 56)?;
    if !(1..=128).contains(&phnum) || phoff < 64 {
        return Err(LoadError::Invalid("program header table"));
    }
    let phbytes = u64::from(phnum) * 56;
    let headers = slice(bytes, phoff, phbytes)?;
    let mut segments = Vec::new();
    let mut phdr = None;
    for header in headers.chunks_exact(56) {
        let kind = u32_at(header, 0)?;
        let flags = u32_at(header, 4)?;
        match kind {
            2 | 3 | 7 => {
                return Err(LoadError::Unsupported(
                    "ELF dynamic linking, interpreter or TLS",
                ));
            }
            0x6474_e551 if flags & 1 != 0 => {
                return Err(LoadError::Unsupported("executable guest stack"));
            }
            1 => {}
            _ => continue,
        }
        let offset = u64_at(header, 8)?;
        let address = u64_at(header, 16)?;
        let filesz = u64_at(header, 32)?;
        let size = u64_at(header, 40)?;
        let alignment = u64_at(header, 48)?;
        if filesz > size
            || offset % PAGE != address % PAGE
            || (alignment > 1
                && (!alignment.is_power_of_two() || offset % alignment != address % alignment))
        {
            return Err(LoadError::Invalid("loadable segment size or alignment"));
        }
        let protection = match flags {
            0 => Protection::None,
            1 => Protection::Execute,
            2 => Protection::Write,
            4 => Protection::Read,
            5 => Protection::ReadExecute,
            6 => Protection::ReadWrite,
            _ => return Err(LoadError::Unsupported("ELF segment permissions")),
        };
        let data = slice(bytes, offset, filesz)?;
        if size == 0 {
            continue;
        }
        if let Some(start) = phoff.checked_sub(offset)
            && start.checked_add(phbytes).is_some_and(|end| end <= filesz)
            && matches!(
                protection,
                Protection::Read | Protection::ReadWrite | Protection::ReadExecute
            )
        {
            phdr = address.checked_add(start);
        }
        segments.push(Segment {
            address: GuestAddress(address),
            size,
            data,
            protection,
        });
    }
    Ok(Elf {
        image: Image { metadata, segments },
        phdr,
        phnum,
    })
}

impl Elf<'_> {
    pub fn start(
        &self,
        args: &[&[u8]],
        environment: &[&[u8]],
    ) -> Result<(AddressSpace, X86State), LoadError> {
        if args.len().saturating_add(environment.len()) > 4096 {
            return Err(LoadError::Unsupported("too many process arguments"));
        }
        let mut memory = self.image.map()?;
        let base = STACK - STACK_SIZE;
        memory.map_guarded_stack(GuestAddress(base - PAGE), STACK_SIZE)?;
        let mut top = STACK;
        let mut words = vec![args.len() as u64];
        for group in [args, environment] {
            for arg in group {
                if arg.contains(&0) {
                    return Err(LoadError::Invalid("NUL in process argument"));
                }
                let len = (arg.len() as u64)
                    .checked_add(1)
                    .ok_or(LoadError::Invalid("initial stack size"))?;
                top = top
                    .checked_sub(len)
                    .filter(|&sp| sp >= base)
                    .ok_or(LoadError::Unsupported("initial stack size"))?;
                memory.write(GuestAddress(top), arg)?;
                words.push(top);
            }
            words.push(0);
        }
        if let Some(phdr) = self.phdr {
            words.extend([3, phdr, 4, 56, 5, u64::from(self.phnum)]);
        }
        words.extend([6, PAGE, 9, self.image.metadata.entry.0, 0, 0]);
        let bytes: Vec<_> = words.into_iter().flat_map(u64::to_le_bytes).collect();
        let sp = top
            .checked_sub(bytes.len() as u64)
            .map(|sp| sp & !15)
            .filter(|&sp| sp >= base)
            .ok_or(LoadError::Unsupported("initial stack size"))?;
        memory.write(GuestAddress(sp), &bytes)?;
        let mut state = X86State {
            rip: self.image.metadata.entry,
            ..X86State::default()
        };
        state.gpr[4] = sp;
        Ok((memory, state))
    }
}
