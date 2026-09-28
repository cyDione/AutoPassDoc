//! The .docx zip container.
//!
//! Keeps the original file bytes so that saving can copy every untouched
//! entry verbatim (still compressed, same metadata). Only parts that were
//! replaced through [`Package::set_part`] are re-encoded.

use std::collections::HashMap;
use std::io::{Cursor, Read, Seek, Write};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::error::{Error, Result};

pub struct Package {
    original: Vec<u8>,
    names: Vec<String>,
    replaced: HashMap<String, Vec<u8>>,
    added: Vec<String>,
}

impl Package {
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self> {
        let archive = ZipArchive::new(Cursor::new(bytes.as_slice()))?;
        let names = archive.file_names().map(str::to_string).collect();
        Ok(Self {
            original: bytes,
            names,
            replaced: HashMap::new(),
            added: Vec::new(),
        })
    }

    pub fn has_part(&self, name: &str) -> bool {
        self.replaced.contains_key(name) || self.names.iter().any(|n| n == name)
    }

    /// Returns the current content of a part (replaced content wins).
    pub fn part(&self, name: &str) -> Result<Option<Vec<u8>>> {
        if let Some(data) = self.replaced.get(name) {
            return Ok(Some(data.clone()));
        }
        if !self.names.iter().any(|n| n == name) {
            return Ok(None);
        }
        let mut archive = ZipArchive::new(Cursor::new(self.original.as_slice()))?;
        let mut file = archive.by_name(name)?;
        let mut data = Vec::with_capacity(file.size() as usize);
        file.read_to_end(&mut data)?;
        Ok(Some(data))
    }

    pub fn required_part(&self, name: &str) -> Result<Vec<u8>> {
        self.part(name)?
            .ok_or_else(|| Error::MissingPart(name.to_string()))
    }

    pub fn set_part(&mut self, name: &str, data: Vec<u8>) {
        if !self.names.iter().any(|n| n == name) && !self.added.iter().any(|n| n == name) {
            self.added.push(name.to_string());
        }
        self.replaced.insert(name.to_string(), data);
    }

    pub fn is_modified(&self) -> bool {
        !self.replaced.is_empty()
    }

    pub fn write_to<W: Write + Seek>(&self, out: W) -> Result<W> {
        let mut archive = ZipArchive::new(Cursor::new(self.original.as_slice()))?;
        let mut writer = ZipWriter::new(out);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        for index in 0..archive.len() {
            let file = archive.by_index_raw(index)?;
            let name = file.name().to_string();
            match self.replaced.get(&name) {
                Some(data) => {
                    drop(file);
                    writer.start_file(name, options)?;
                    writer.write_all(data)?;
                }
                None => writer.raw_copy_file(file)?,
            }
        }
        for name in &self.added {
            writer.start_file(name.as_str(), options)?;
            writer.write_all(&self.replaced[name])?;
        }
        Ok(writer.finish()?)
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        Ok(self.write_to(Cursor::new(Vec::new()))?.into_inner())
    }
}
