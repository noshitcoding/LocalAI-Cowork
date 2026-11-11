//! Portable in-memory ZIP boundary tests; no filesystem extraction or external services.
use cowork_domain::Error;
use cowork_storage::{PRODUCTIVITY_ARCHIVE_MAX_BYTES, ProductivityArchive};
use std::io::{Cursor, Write};
use zip::{CompressionMethod, ZipArchive, ZipWriter, write::SimpleFileOptions};

fn archive() -> ProductivityArchive {
    ProductivityArchive {
        format: "localai-cowork-product".into(),
        version: 1,
        created_at: 123,
        projects: Vec::new(),
        personal_knowledge_folders: Vec::new(),
    }
}

fn body() -> Vec<u8> {
    serde_json::to_vec(&archive()).unwrap()
}

fn zip_entries(entries: &[(&str, &[u8])], method: CompressionMethod) -> Vec<u8> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let mut options = SimpleFileOptions::default().compression_method(method);
    if method == CompressionMethod::Deflated {
        // Large boundary fixtures need ordinary fast DEFLATE, not expensive compression.
        options = options.compression_level(Some(1));
    }
    for (name, data) in entries {
        writer.start_file(*name, options).unwrap();
        writer.write_all(data).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

fn assert_validation_rejected(bytes: &[u8]) {
    assert!(matches!(
        ProductivityArchive::from_zip(bytes),
        Err(Error::Validation(_))
    ));
}

#[test]
fn deflate_export_roundtrip_preserves_format_one_archive() {
    let expected = archive();
    let bytes = expected.to_zip().unwrap();
    let mut reader = ZipArchive::new(Cursor::new(bytes.as_slice())).unwrap();
    assert_eq!(reader.len(), 1);
    assert_eq!(
        reader.by_index(0).unwrap().compression(),
        CompressionMethod::Deflated
    );
    let actual = ProductivityArchive::from_zip(&bytes).unwrap();
    assert_eq!(
        serde_json::to_value(actual).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
}

// Fixed external-producer fixture: CPython 3.14 stdlib zipfile, DEFLATE level6,
// manifest.json, timestamp1980-01-01, regular0644; no writer under test generated it.
// ZIP SHA256: ba4fac7d30a9f6d69ee781e6963915a9c5fa14f653aceadbc68d088e35a43656.
const FIXED_FORMAT_ONE_ZIP: &[u8] = &[
    0x50, 0x4b, 0x03, 0x04, 0x14, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x21, 0x00, 0x00, 0x35,
    0xec, 0xbe, 0x4c, 0x00, 0x00, 0x00, 0x4e, 0x00, 0x00, 0x00, 0x0d, 0x00, 0x00, 0x00, 0x6d, 0x61,
    0x6e, 0x69, 0x66, 0x65, 0x73, 0x74, 0x2e, 0x6a, 0x73, 0x6f, 0x6e, 0x15, 0xcb, 0x31, 0x0a, 0x80,
    0x30, 0x0c, 0x05, 0xd0, 0xbb, 0xfc, 0xb9, 0x0e, 0xd5, 0xad, 0x57, 0x11, 0x91, 0x90, 0x56, 0xa8,
    0x56, 0x53, 0xd2, 0xa8, 0x83, 0x78, 0x77, 0xf1, 0xed, 0xef, 0xc1, 0x22, 0xba, 0x93, 0x21, 0xa0,
    0x08, 0x53, 0xa1, 0xdc, 0xb1, 0xdc, 0xa2, 0x5b, 0x57, 0x55, 0xe2, 0xc9, 0x06, 0x87, 0x2b, 0x69,
    0xcb, 0x72, 0x20, 0x78, 0x07, 0xd6, 0x44, 0x96, 0xe2, 0xfc, 0x0f, 0xdf, 0x0f, 0x0e, 0x55, 0x65,
    0x4d, 0x6c, 0x0d, 0x61, 0x9c, 0xde, 0x0f, 0x50, 0x4b, 0x01, 0x02, 0x14, 0x00, 0x14, 0x00, 0x00,
    0x00, 0x08, 0x00, 0x00, 0x00, 0x21, 0x00, 0x00, 0x35, 0xec, 0xbe, 0x4c, 0x00, 0x00, 0x00, 0x4e,
    0x00, 0x00, 0x00, 0x0d, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xa4,
    0x81, 0x00, 0x00, 0x00, 0x00, 0x6d, 0x61, 0x6e, 0x69, 0x66, 0x65, 0x73, 0x74, 0x2e, 0x6a, 0x73,
    0x6f, 0x6e, 0x50, 0x4b, 0x05, 0x06, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x3b, 0x00,
    0x00, 0x00, 0x77, 0x00, 0x00, 0x00, 0x00, 0x00,
];

#[test]
fn reads_fixed_external_format_one_deflate_archive() {
    let actual = ProductivityArchive::from_zip(FIXED_FORMAT_ONE_ZIP).unwrap();
    assert_eq!(
        serde_json::to_value(actual).unwrap(),
        serde_json::to_value(archive()).unwrap()
    );
}

#[test]
fn wrong_entry_name_is_rejected() {
    let bytes = zip_entries(&[("other.json", &body())], CompressionMethod::Deflated);
    assert_validation_rejected(&bytes);
}

#[test]
fn additional_entry_is_rejected() {
    let manifest = body();
    let bytes = zip_entries(
        &[("manifest.json", &manifest), ("extra.txt", b"extra")],
        CompressionMethod::Deflated,
    );
    assert_validation_rejected(&bytes);
}

#[test]
fn symlink_manifest_with_otherwise_valid_json_is_rejected() {
    let manifest = body();
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    // A symlink's payload is its target text. Keep it valid manifest JSON so a
    // missing symlink rejection could not pass merely because JSON parsing fails.
    writer
        .add_symlink(
            "manifest.json",
            std::str::from_utf8(&manifest).unwrap(),
            SimpleFileOptions::default(),
        )
        .unwrap();
    let bytes = writer.finish().unwrap().into_inner();
    let mut reader = ZipArchive::new(Cursor::new(bytes.as_slice())).unwrap();
    assert_eq!(
        reader.by_index(0).unwrap().unix_mode().unwrap() & 0o170000,
        0o120000
    );
    assert_validation_rejected(&bytes);
}

#[test]
fn valid_stored_zip_exceeding_compressed_input_limit_is_rejected() {
    let mut manifest = body();
    // JSON whitespace preserves a valid manifest at exactly the output limit.
    // Stored ZIP headers alone then put the valid input beyond the input limit.
    manifest.resize(PRODUCTIVITY_ARCHIVE_MAX_BYTES, b' ');
    let bytes = zip_entries(&[("manifest.json", &manifest)], CompressionMethod::Stored);
    assert!(bytes.len() > PRODUCTIVITY_ARCHIVE_MAX_BYTES);
    let mut reader = ZipArchive::new(Cursor::new(bytes.as_slice())).unwrap();
    assert_eq!(
        reader.by_index(0).unwrap().size(),
        PRODUCTIVITY_ARCHIVE_MAX_BYTES as u64
    );
    assert_validation_rejected(&bytes);
}

#[test]
fn compressed_manifest_at_output_limit_is_accepted() {
    let mut manifest = body();
    manifest.resize(PRODUCTIVITY_ARCHIVE_MAX_BYTES, b' ');
    let bytes = zip_entries(&[("manifest.json", &manifest)], CompressionMethod::Deflated);
    assert!(bytes.len() < PRODUCTIVITY_ARCHIVE_MAX_BYTES);
    let actual = ProductivityArchive::from_zip(&bytes).unwrap();
    assert_eq!(
        serde_json::to_value(actual).unwrap(),
        serde_json::to_value(archive()).unwrap()
    );
}

#[test]
fn small_compressed_zip_exceeding_output_limit_is_rejected() {
    let mut manifest = body();
    manifest.resize(PRODUCTIVITY_ARCHIVE_MAX_BYTES + 1, b' ');
    let bytes = zip_entries(&[("manifest.json", &manifest)], CompressionMethod::Deflated);
    assert!(bytes.len() < PRODUCTIVITY_ARCHIVE_MAX_BYTES);
    let mut reader = ZipArchive::new(Cursor::new(bytes.as_slice())).unwrap();
    assert_eq!(
        reader.by_index(0).unwrap().size(),
        PRODUCTIVITY_ARCHIVE_MAX_BYTES as u64 + 1
    );
    assert_validation_rejected(&bytes);
}

#[test]
fn malformed_manifest_json_is_rejected() {
    let bytes = zip_entries(&[("manifest.json", b"{")], CompressionMethod::Deflated);
    assert_validation_rejected(&bytes);
}

#[test]
fn invalid_utf8_raw_entry_name_is_rejected_after_decode_fallback() {
    let mut bytes = zip_entries(
        &[("manif\u{00e9}st.json", &body())],
        CompressionMethod::Deflated,
    );
    let (local, central) = {
        let mut reader = ZipArchive::new(Cursor::new(bytes.as_slice())).unwrap();
        let entry = reader.by_index(0).unwrap();
        (
            entry.header_start() as usize,
            entry.central_header_start() as usize,
        )
    };
    assert_eq!(&bytes[local..local + 4], b"PK\x03\x04");
    assert_eq!(&bytes[central..central + 4], b"PK\x01\x02");
    // The standard UTF-8 bit is set by the external-facing writer for this name.
    assert_ne!(
        u16::from_le_bytes([bytes[local + 6], bytes[local + 7]]) & 0x0800,
        0
    );
    assert_ne!(
        u16::from_le_bytes([bytes[central + 8], bytes[central + 9]]) & 0x0800,
        0
    );
    // Preserve equal local/central raw names while making their UTF-8 invalid.
    // ZIP fixed-header filename offsets are30 and46; contents/CRC are unchanged.
    bytes[local + 30] = 0xff;
    bytes[central + 46] = 0xff;
    let mut reader = ZipArchive::new(Cursor::new(bytes.as_slice())).unwrap();
    let entry = reader.by_index(0).unwrap();
    // zip9.0.5 falls back to CP437 even when invalid raw UTF-8 is flagged as UTF-8.
    // Prove malformed raw bytes and reject the decoded non-manifest name.
    assert!(std::str::from_utf8(entry.name_raw()).is_err());
    assert_ne!(entry.name().unwrap(), "manifest.json");
    assert_validation_rejected(&bytes);
}
