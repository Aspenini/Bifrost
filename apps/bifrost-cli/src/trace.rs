use bifrost_runtime::core::{FaultClass, TraceEvent, TraceSink};
use std::{
    fs::File,
    io::{self, BufWriter, Write},
    path::Path,
};

pub struct JsonLines {
    file: BufWriter<File>,
    error: Option<io::Error>,
}

impl JsonLines {
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = File::options().write(true).create_new(true).open(path)?;
        let mut trace = Self {
            file: BufWriter::new(file),
            error: None,
        };
        writeln!(trace.file, "{{\"event\":\"start\",\"schema\":1}}")?;
        Ok(trace)
    }

    pub fn finish(mut self, outcome: &Result<u32, super::Error>) -> io::Result<()> {
        if let Some(error) = self.error.take() {
            return Err(error);
        }
        match outcome {
            Ok(status) => writeln!(self.file, "{{\"event\":\"exit\",\"status\":{status}}}")?,
            Err(_) => writeln!(self.file, "{{\"event\":\"error\"}}")?,
        }
        self.file.flush()
    }

    fn write(&mut self, event: TraceEvent) -> io::Result<()> {
        match event {
            TraceEvent::Step { thread, pc } => writeln!(
                self.file,
                "{{\"event\":\"step\",\"thread\":{},\"pc\":\"{:#x}\"}}",
                thread.0, pc.0
            ),
            TraceEvent::Fault(fault) => {
                write!(
                    self.file,
                    "{{\"event\":\"fault\",\"thread\":{},\"pc\":\"{:#x}\"",
                    fault.thread.0, fault.pc.0
                )?;
                match fault.class {
                    FaultClass::Trap(code) => {
                        write!(self.file, ",\"class\":\"Trap\",\"trap\":{code}")?
                    }
                    class => write!(self.file, ",\"class\":\"{class:?}\"")?,
                }
                if let Some((address, access)) = fault.memory_access {
                    write!(
                        self.file,
                        ",\"address\":\"{:#x}\",\"access\":\"{access:?}\"",
                        address.0
                    )?;
                }
                writeln!(self.file, "}}")
            }
            _ => Ok(()),
        }
    }
}

impl TraceSink for JsonLines {
    fn record(&mut self, event: TraceEvent) {
        if self.error.is_none()
            && let Err(error) = self.write(event)
        {
            self.error = Some(error);
        }
    }
}
