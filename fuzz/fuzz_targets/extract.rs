#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = fast_docx_text::declared_xml_bytes(data);
    let _ = fast_docx_text::extract_raw_text(data);
});
