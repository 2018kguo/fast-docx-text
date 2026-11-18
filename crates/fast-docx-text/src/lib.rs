//! Plain text from a `.docx`, byte-identical to mammoth's `extractRawText`.
//!
//! [`extract_raw_text`] reads the package the way mammoth does: JSZip's reading of the
//! archive ([`zip`]), @xmldom/xmldom's reading of each part ([`xmldom`]) and mammoth's
//! document model and raw-text writer ([`raw_text`]). It also fails where mammoth
//! fails, so a file one refuses the other refuses too.
//!
//! Every input is untrusted: a part inflates to at most [`zip::MAX_PART_BYTES`] and
//! never past the size the archive declares for it, XML nests at most
//! [`xmldom::MAX_DEPTH`] levels, and every failure is an `Err`, never a panic.

pub mod raw_text;
mod dingbats;
pub mod tree;
pub mod xmldom;
pub mod zip;

/// The document's text: each paragraph followed by a blank line, as mammoth writes it.
/// The error is the message mammoth's own failure would carry, or close to it.
pub fn extract_raw_text(data: &[u8]) -> Result<String, String> {
    raw_text::extract(data)
}

/// What the archive declares the parts a read may parse inflate to, together: every
/// `.xml` and `.rels` part, and every part reached through relationships whatever its
/// name (the main document may be `word/document.dat`). An upper bound on the XML a
/// read parses (saturating; 0 when the archive cannot be opened, which a read reports
/// at once). Reads the zip directory and the package's relationship parts only.
pub fn declared_xml_bytes(data: &[u8]) -> u64 {
    let Ok(package) = zip::open(data) else {
        return 0;
    };
    let named_xml = |name: &str| {
        let name = name.to_lowercase();
        name.ends_with(".xml") || name.ends_with(".rels")
    };
    let declared = package
        .entries
        .iter()
        .filter(|entry| !entry.dir && named_xml(&entry.name))
        .fold(0u64, |sum, entry| sum.saturating_add(entry.declared_size()));
    // Relationship parts over 1 MiB are not read here: they are already counted.
    let mut related = raw_text::related_parts(&package, 1 << 20);
    related.retain(|path| !named_xml(path));
    related.sort();
    related.dedup();
    related
        .iter()
        .filter_map(|path| package.file(path))
        .fold(declared, |sum, entry| sum.saturating_add(entry.declared_size()))
}

/// The WebAssembly interface, a plain C ABI with no imports: JavaScript copies the file
/// into [`alloc`]ated memory, calls [`extract`] or [`declared_xml_bytes`], and reads the
/// UTF-8 result through [`output_ptr`] and [`output_len`]. The JavaScript package
/// instantiates the module afresh for every call, so nothing here outlives one call.
#[cfg(target_arch = "wasm32")]
mod wasm {
    static mut OUTPUT: Vec<u8> = Vec::new();

    /// A buffer of `len` bytes for the input; [`extract`] takes ownership of it.
    #[unsafe(no_mangle)]
    pub extern "C" fn alloc(len: usize) -> *mut u8 {
        let mut buffer = Vec::<u8>::with_capacity(len);
        let pointer = buffer.as_mut_ptr();
        std::mem::forget(buffer);
        pointer
    }

    fn take_input(pointer: *mut u8, len: usize) -> Vec<u8> {
        // SAFETY: `pointer` came from `alloc(len)` and JavaScript filled all `len` bytes.
        unsafe { Vec::from_raw_parts(pointer, len, len) }
    }

    /// Returns 0 with the text as output, or 1 with the error message as output.
    #[unsafe(no_mangle)]
    pub extern "C" fn extract(pointer: *mut u8, len: usize) -> u32 {
        let data = take_input(pointer, len);
        let (status, output) = match super::extract_raw_text(&data) {
            Ok(text) => (0, text),
            Err(message) => (1, message),
        };
        // SAFETY: single-threaded module; nothing else touches OUTPUT during a call.
        unsafe { OUTPUT = output.into_bytes() };
        status
    }

    /// [`super::declared_xml_bytes`], saturated to u32.
    #[unsafe(no_mangle)]
    pub extern "C" fn declared_xml_bytes(pointer: *mut u8, len: usize) -> u32 {
        let data = take_input(pointer, len);
        u32::try_from(super::declared_xml_bytes(&data)).unwrap_or(u32::MAX)
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn output_ptr() -> *const u8 {
        // SAFETY: see `extract`.
        unsafe { (*std::ptr::addr_of!(OUTPUT)).as_ptr() }
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn output_len() -> usize {
        // SAFETY: see `extract`.
        unsafe { (*std::ptr::addr_of!(OUTPUT)).len() }
    }
}
