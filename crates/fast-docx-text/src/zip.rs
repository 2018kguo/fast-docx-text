//! The package reader both JavaScript readers used: JSZip 3.10.1's `loadAsync`, entry for
//! entry. Member names come from the local headers and pass through JSZip's `resolve`; a
//! later member of the same name replaces an earlier one; a part is inflated only when a
//! reader asks for it, and its length must be the one the central directory declared.
//!
//! Untrusted input: every offset is bounds-checked, a part may inflate to at most
//! [`MAX_PART_BYTES`], and inflation stops at the declared size, so a zip bomb costs no
//! more than the cap.

use std::collections::HashMap;

use miniz_oxide::inflate::TINFLStatus;
use miniz_oxide::inflate::core::{DecompressorOxide, decompress, inflate_flags};

/// The most a single part may inflate to. A 2,200-page document.xml is about 19 MB.
pub const MAX_PART_BYTES: usize = 128 * 1024 * 1024;

const LOCAL_FILE_HEADER: u32 = 0x0403_4b50;
const CENTRAL_FILE_HEADER: u32 = 0x0201_4b50;
const CENTRAL_DIRECTORY_END: u32 = 0x0605_4b50;
const ZIP64_CENTRAL_DIRECTORY_LOCATOR: u32 = 0x0706_4b50;
const ZIP64_CENTRAL_DIRECTORY_END: u32 = 0x0606_4b50;

pub type Error = String;

struct Reader<'a> {
    data: &'a [u8],
    index: usize,
    /// Bytes before the archive proper (JSZip's `zero`).
    zero: usize,
}

impl<'a> Reader<'a> {
    /// Offsets and sizes come from the file, so sums saturate: on wasm32 a wrapped
    /// sum would pass this check and then panic slicing.
    fn check(&self, end: usize) -> Result<(), Error> {
        if self.zero.saturating_add(end) > self.data.len() {
            return Err(format!(
                "End of data reached (data length = {}, asked index = {}). Corrupted zip ?",
                self.data.len(),
                end
            ));
        }
        Ok(())
    }
    fn set_index(&mut self, index: usize) -> Result<(), Error> {
        self.check(index)?;
        self.index = index;
        Ok(())
    }
    fn skip(&mut self, n: usize) -> Result<(), Error> {
        self.set_index(self.index.saturating_add(n))
    }
    fn read(&mut self, n: usize) -> Result<&'a [u8], Error> {
        self.check(self.index.saturating_add(n))?;
        let start = self.zero + self.index;
        self.index += n;
        Ok(&self.data[start..start + n])
    }
    fn int(&mut self, n: usize) -> Result<u64, Error> {
        let bytes = self.read(n)?;
        Ok(bytes
            .iter()
            .rev()
            .fold(0u64, |acc, &byte| (acc << 8) | u64::from(byte)))
    }
    fn signature_at(&self, at: usize, signature: u32) -> bool {
        let start = self.zero.saturating_add(at);
        self.data
            .get(start..start.saturating_add(4))
            .is_some_and(|bytes| bytes == signature.to_le_bytes())
    }
    fn last_index_of(&self, signature: u32) -> Option<usize> {
        memchr::memmem::rfind(&self.data[self.zero..], &signature.to_le_bytes())
    }
}

/// One member as the central directory and its local header describe it.
pub struct Entry<'a> {
    /// JSZip's `fileNameStr`.
    pub name: String,
    pub dir: bool,
    method: u16,
    uncompressed_size: u64,
    data: &'a [u8],
}

impl Entry<'_> {
    /// What the central directory says the part inflates to. [`Entry::bytes`] never
    /// inflates more than this (one byte past it fails), so it is a true upper bound.
    pub fn declared_size(&self) -> u64 {
        self.uncompressed_size
    }

    /// The part's bytes, inflated, exactly as long as declared.
    pub fn bytes(&self) -> Result<Vec<u8>, Error> {
        let size = usize::try_from(self.uncompressed_size).unwrap_or(usize::MAX);
        if size > MAX_PART_BYTES {
            return Err(format!(
                "Refusing to inflate {}: it declares {} bytes, past the {}-byte part cap",
                self.name, self.uncompressed_size, MAX_PART_BYTES
            ));
        }
        let out = match self.method {
            0 => self.data.to_vec(),
            _ => inflate(self.data, size)?,
        };
        if out.len() != size {
            return Err("Bug : uncompressed data size mismatch".into());
        }
        Ok(out)
    }
}

/// Raw DEFLATE into a buffer of the declared size plus one byte, so a stream that would
/// produce more than declared stops there and fails the length check.
fn inflate(data: &[u8], size: usize) -> Result<Vec<u8>, Error> {
    let mut out = vec![0u8; size + 1];
    let mut decompressor = DecompressorOxide::new();
    let flags = inflate_flags::TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF;
    let (status, _, written) = decompress(&mut decompressor, data, &mut out, 0, flags);
    match status {
        TINFLStatus::Done => {}
        // More output than declared: the length check reports it.
        TINFLStatus::HasMoreOutput => {}
        TINFLStatus::NeedsMoreInput | TINFLStatus::FailedCannotMakeProgress => {
            return Err("invalid deflate data: unexpected end of stream".into());
        }
        _ => return Err("invalid deflate data".into()),
    }
    out.truncate(written);
    Ok(out)
}

/// A loaded package: every member in central-directory order, and JSZip's name → member
/// map after `resolve` and replacement.
pub struct Package<'a> {
    pub entries: Vec<Entry<'a>>,
    files: HashMap<String, usize>,
}

impl<'a> Package<'a> {
    /// JSZip's `zip.file(name)`: a member that is not a directory.
    pub fn file(&self, name: &str) -> Option<&Entry<'a>> {
        self.files
            .get(name)
            .map(|&index| &self.entries[index])
            .filter(|entry| !entry.dir)
    }

}

/// JSZip's `utils.resolve`: drops `.` and inner empty segments, and `..` pops one.
fn resolve(path: &str) -> String {
    let parts: Vec<&str> = path.split('/').collect();
    let last = parts.len() - 1;
    let mut result: Vec<&str> = Vec::new();
    for (index, part) in parts.iter().enumerate() {
        if *part == "." || (part.is_empty() && index != 0 && index != last) {
            continue;
        } else if *part == ".." {
            result.pop();
        } else {
            result.push(part);
        }
    }
    result.join("/")
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xEDB8_8320 & (!(crc & 1)).wrapping_add(1));
        }
    }
    !crc
}

pub fn open(data: &[u8]) -> Result<Package<'_>, Error> {
    let mut reader = Reader {
        data,
        index: 0,
        zero: 0,
    };
    let Some(end_offset) = reader.last_index_of(CENTRAL_DIRECTORY_END) else {
        return Err(if reader.signature_at(0, LOCAL_FILE_HEADER) {
            "Corrupted zip: can't find end of central directory".into()
        } else {
            "Can't find end of central directory : is this a zip file ?".into()
        });
    };
    reader.set_index(end_offset + 4)?;
    let disk_number = reader.int(2)?;
    let disk_with_start = reader.int(2)?;
    let records_on_disk = reader.int(2)?;
    let mut records = reader.int(2)?;
    let mut directory_size = reader.int(4)?;
    let mut directory_offset = reader.int(4)?;
    let comment_length = reader.int(2)? as usize;
    reader.read(comment_length)?;

    let zip64 = disk_number == 0xFFFF
        || disk_with_start == 0xFFFF
        || records_on_disk == 0xFFFF
        || records == 0xFFFF
        || directory_size == 0xFFFF_FFFF
        || directory_offset == 0xFFFF_FFFF;
    let mut zip64_end_size = 0;
    if zip64 {
        let Some(locator) = reader.last_index_of(ZIP64_CENTRAL_DIRECTORY_LOCATOR) else {
            return Err(
                "Corrupted zip: can't find the ZIP64 end of central directory locator".into(),
            );
        };
        reader.set_index(locator.saturating_add(4))?;
        reader.int(4)?;
        let mut zip64_end = reader.int(8)? as usize;
        if reader.int(4)? > 1 {
            return Err("Multi-volumes zip are not supported".into());
        }
        if !reader.signature_at(zip64_end, ZIP64_CENTRAL_DIRECTORY_END) {
            zip64_end = reader
                .last_index_of(ZIP64_CENTRAL_DIRECTORY_END)
                .ok_or("Corrupted zip: can't find the ZIP64 end of central directory")?;
        }
        reader.set_index(zip64_end.saturating_add(4))?;
        zip64_end_size = reader.int(8)?;
        reader.skip(4)?;
        reader.int(4)?;
        reader.int(4)?;
        reader.int(8)?;
        records = reader.int(8)?;
        directory_size = reader.int(8)?;
        directory_offset = reader.int(8)?;
    }

    let mut expected_end = directory_offset.saturating_add(directory_size);
    if zip64 {
        expected_end = expected_end.saturating_add(32).saturating_add(zip64_end_size);
    }
    let end_offset = end_offset as u64;
    if end_offset > expected_end {
        if !reader.signature_at(end_offset as usize, CENTRAL_FILE_HEADER) {
            reader.zero = (end_offset - expected_end) as usize;
        }
    } else if end_offset < expected_end {
        return Err(format!(
            "Corrupted zip: missing {} bytes.",
            expected_end - end_offset
        ));
    }

    struct Central<'a> {
        made_by: u64,
        flag: u64,
        method: u16,
        compressed_size: u64,
        uncompressed_size: u64,
        external: u64,
        local_offset: u64,
        extra: HashMap<u64, &'a [u8]>,
    }
    let mut central = Vec::new();
    reader.set_index(usize::try_from(directory_offset).map_err(|_| "Corrupted zip")?)?;
    while reader.int(4)? == u64::from(CENTRAL_FILE_HEADER) {
        let made_by = reader.int(2)?;
        reader.skip(2)?;
        let flag = reader.int(2)?;
        let method = reader.int(2)? as u16;
        reader.int(4)?;
        reader.int(4)?;
        let mut compressed_size = reader.int(4)?;
        let mut uncompressed_size = reader.int(4)?;
        let name_length = reader.int(2)? as usize;
        let extra_length = reader.int(2)? as usize;
        let comment_length = reader.int(2)? as usize;
        let mut disk_start = reader.int(2)?;
        reader.int(2)?;
        let external = reader.int(4)?;
        let mut local_offset = reader.int(4)?;
        if flag & 1 == 1 {
            return Err("Encrypted zip are not supported".into());
        }
        reader.skip(name_length)?;
        let extra_end = reader.index + extra_length;
        let mut extra = HashMap::new();
        while reader.index + 4 < extra_end {
            let id = reader.int(2)?;
            let length = reader.int(2)? as usize;
            extra.insert(id, reader.read(length)?);
        }
        reader.set_index(extra_end)?;
        if zip64 && let Some(&field) = extra.get(&1) {
            let mut field_reader = Reader {
                data: field,
                index: 0,
                zero: 0,
            };
            if uncompressed_size == 0xFFFF_FFFF {
                uncompressed_size = field_reader.int(8)?;
            }
            if compressed_size == 0xFFFF_FFFF {
                compressed_size = field_reader.int(8)?;
            }
            if local_offset == 0xFFFF_FFFF {
                local_offset = field_reader.int(8)?;
            }
            if disk_start == 0xFFFF_FFFF {
                disk_start = field_reader.int(4)?;
            }
        }
        let _ = disk_start;
        reader.read(comment_length)?;
        central.push(Central {
            made_by,
            flag,
            method,
            compressed_size,
            uncompressed_size,
            external,
            local_offset,
            extra,
        });
    }
    if records != central.len() as u64 && records != 0 && central.is_empty() {
        return Err(format!(
            "Corrupted zip or bug: expected {records} records in central dir, got 0"
        ));
    }

    let mut entries = Vec::with_capacity(central.len());
    for file in central {
        reader.set_index(usize::try_from(file.local_offset).map_err(|_| "Corrupted zip")?)?;
        if reader.int(4)? != u64::from(LOCAL_FILE_HEADER) {
            return Err("Corrupted zip or bug: unexpected signature".into());
        }
        reader.skip(22)?;
        let name_length = reader.int(2)? as usize;
        let extra_length = reader.int(2)? as usize;
        let local_name = reader.read(name_length)?;
        reader.skip(extra_length)?;
        if file.method != 0 && file.method != 8 {
            return Err(format!(
                "Corrupted zip : compression {:#06x} unknown (inner file : {})",
                file.method,
                String::from_utf8_lossy(local_name)
            ));
        }
        let data = reader
            .read(usize::try_from(file.compressed_size).map_err(|_| "Corrupted zip")?)?;
        let utf8 = file.flag & 0x0800 != 0;
        let name = if utf8 {
            String::from_utf8_lossy(local_name).into_owned()
        } else {
            unicode_path(&file.extra, local_name)
                .unwrap_or_else(|| String::from_utf8_lossy(local_name).into_owned())
        };
        let dir = file.external & 0x10 != 0 || name.ends_with('/');
        let _ = file.made_by;
        entries.push(Entry {
            name,
            dir,
            method: file.method,
            uncompressed_size: file.uncompressed_size,
            data,
        });
    }

    let mut files = HashMap::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let mut key = resolve(&entry.name);
        if entry.dir && !key.ends_with('/') {
            key.push('/');
        }
        files.insert(key, index);
    }
    Ok(Package { entries, files })
}

/// The Info-ZIP Unicode Path extra field, when it vouches for this name.
fn unicode_path(extra: &HashMap<u64, &[u8]>, name: &[u8]) -> Option<String> {
    let field = extra.get(&0x7075)?;
    if field.len() < 5 || field[0] != 1 {
        return None;
    }
    let crc = u32::from_le_bytes([field[1], field[2], field[3], field[4]]);
    (crc == crc32(name)).then(|| String::from_utf8_lossy(&field[5..]).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_matches_jszip() {
        assert_eq!(resolve("word/./document.xml"), "word/document.xml");
        assert_eq!(resolve("a/../word/document.xml"), "word/document.xml");
        assert_eq!(resolve("/abs//x/"), "/abs/x/");
    }

    #[test]
    fn crc32_known_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }
}
