//! Explicit read-only guest mounts. Paths never resolve through the host filesystem.
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PathError {
    Invalid,
    Duplicate,
}
impl std::fmt::Display for PathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "guest mount path: {self:?}")
    }
}
impl std::error::Error for PathError {}

#[derive(Default)]
pub struct Vfs {
    files: BTreeMap<Vec<u8>, Arc<[u8]>>,
}

impl Vfs {
    pub fn mount(&mut self, path: &[u8], bytes: Vec<u8>) -> Result<(), PathError> {
        let path = normalize(path)?;
        match self.files.entry(path) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(bytes.into());
                Ok(())
            }
            std::collections::btree_map::Entry::Occupied(_) => Err(PathError::Duplicate),
        }
    }

    pub fn open(&self, path: &[u8]) -> Result<Option<Arc<[u8]>>, PathError> {
        Ok(self.files.get(&normalize(path)?).cloned())
    }
}

fn normalize(path: &[u8]) -> Result<Vec<u8>, PathError> {
    if !path.starts_with(b"/") || path.len() > 4096 || path.contains(&0) || path.ends_with(b"/") {
        return Err(PathError::Invalid);
    }
    let mut out = Vec::new();
    for component in path.split(|&byte| byte == b'/') {
        match component {
            b"" | b"." => continue,
            b".." => return Err(PathError::Invalid),
            _ => {}
        }
        out.push(b'/');
        out.extend(component);
    }
    if out.is_empty() {
        Err(PathError::Invalid)
    } else {
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_explicit_guest_paths_resolve() {
        let mut fs = Vfs::default();
        fs.mount(b"/fixtures/hello", vec![42]).unwrap();
        assert_eq!(&*fs.open(b"/fixtures//./hello").unwrap().unwrap(), [42]);
        assert!(matches!(
            fs.open(b"/fixtures/../secret"),
            Err(PathError::Invalid)
        ));
        assert!(fs.open(b"/fixtures/missing").unwrap().is_none());
        assert_eq!(
            fs.mount(b"/fixtures/./hello", vec![43]),
            Err(PathError::Duplicate)
        );
    }
}
