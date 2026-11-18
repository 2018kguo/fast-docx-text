//! A bounded, seeded fuzz of the zip and XML layers on stable Rust: small valid
//! packages, damaged at random (bit flips, truncation, spliced bytes, broken sizes
//! and offsets, XML cut and stitched), must each read or fail with an `Err`, never
//! panic. `fuzz/` has the coverage-guided libFuzzer target for longer runs.
//!
//!   MUTATIONS=1000000 cargo test --release --test mutations

use fast_docx_text::{declared_xml_bytes, extract_raw_text};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

/// A stored (uncompressed) zip of `files`.
fn zip(files: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in files {
        let offset = out.len() as u32;
        let crc = crc32(data);
        let header = |signature: u32, out: &mut Vec<u8>| {
            out.extend_from_slice(&signature.to_le_bytes());
        };
        header(0x0403_4b50, &mut out);
        out.extend_from_slice(&[20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);
        header(0x0201_4b50, &mut central);
        central.extend_from_slice(&[20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        central.extend_from_slice(&(name.len() as u16).to_le_bytes());
        central.extend_from_slice(&[0; 12]);
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let directory_offset = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&directory_offset.to_le_bytes());
    out.extend_from_slice(&[0, 0]);
    out
}

const BODIES: &[&str] = &[
    r#"<w:p><w:r><w:t>Hello</w:t><w:tab/><w:t xml:space="preserve"> world </w:t></w:r></w:p>"#,
    r#"<w:tbl><w:tr><w:tc><w:tcPr><w:gridSpan w:val="2"/><w:vMerge/></w:tcPr><w:p><w:r><w:t>a</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#,
    r#"<w:p><w:ins><w:r><w:t>new</w:t></w:r></w:ins><w:del><w:r><w:delText>old</w:delText></w:r></w:del><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText> PAGE </w:instrText></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#,
    r#"<w:sdt><w:sdtPr><w14:checkbox/></w:sdtPr><w:sdtContent><w:p><w:r><w:t>&amp;&#x41;&lt;</w:t></w:r></w:p></w:sdtContent></w:sdt><mc:AlternateContent><mc:Fallback><w:p><w:r><w:sym w:font="Symbol" w:char="F0B7"/></w:r></w:p></mc:Fallback></mc:AlternateContent>"#,
];

fn package(rng: &mut Rng) -> Vec<u8> {
    let body = BODIES[rng.below(BODIES.len())];
    let mut document = format!(
        r#"<?xml version="1.0"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"><w:body>{body}</w:body></w:document>"#
    )
    .into_bytes();
    // Damage the XML itself sometimes: cut it, or stitch a piece of it elsewhere.
    match rng.below(4) {
        0 => document.truncate(rng.below(document.len())),
        1 => {
            let from = rng.below(document.len());
            let piece = document[from..(from + rng.below(40)).min(document.len())].to_vec();
            let at = rng.below(document.len());
            document.splice(at..at, piece);
        }
        _ => {}
    }
    zip(&[
        (
            "[Content_Types].xml",
            br#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" ContentType="application/xml"/></Types>"#.to_vec(),
        ),
        (
            "_rels/.rels",
            br#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#.to_vec(),
        ),
        ("word/document.xml", document),
    ])
}

fn mutate(rng: &mut Rng, bytes: &mut Vec<u8>) {
    for _ in 0..1 + rng.below(4) {
        if bytes.is_empty() {
            return;
        }
        let at = rng.below(bytes.len());
        match rng.below(6) {
            0 => bytes[at] ^= 1 << rng.below(8),
            1 => bytes.truncate(at),
            2 => bytes[at] = 0xFF,
            // A 32-bit size or offset field set to something large or zero.
            3 if at + 4 <= bytes.len() => {
                let value = [0u32, 0xFFFF_FFF0, 0x7FFF_FFFF, rng.next() as u32][rng.below(4)];
                bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
            }
            4 => {
                let from = rng.below(bytes.len());
                let piece = bytes[from..(from + rng.below(64)).min(bytes.len())].to_vec();
                bytes.splice(at..at, piece);
            }
            _ => {
                bytes.remove(at);
            }
        }
    }
}

#[test]
fn damaged_packages_never_panic() {
    let cases: u64 = std::env::var("MUTATIONS").ok().and_then(|n| n.parse().ok()).unwrap_or(20_000);
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut read = 0;
    for _ in 0..cases {
        let mut bytes = package(&mut rng);
        if rng.below(4) != 0 {
            mutate(&mut rng, &mut bytes);
        }
        let _ = declared_xml_bytes(&bytes);
        if extract_raw_text(&bytes).is_ok() {
            read += 1;
        }
    }
    // Both outcomes must stay exercised.
    assert!(read > 0 && read < cases, "{read} of {cases} read");
}
