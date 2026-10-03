//! Web font registration: sfnt (TTF/OTF) utilities.
//!
//! `@font-face` declares a family name that CSS `font-family` matches —
//! independent of the font file's internal name table. fontdb indexes
//! fonts by their internal family, so we rebuild the `name` table with the
//! declared family (Chrome behaves the same way: the CSS name wins).

/// Rebuild a raw sfnt font so its family name (nameID 1/4/6/16) is
/// `family`. Table data is copied unchanged; only the name table is
/// replaced (checksums are not validated by ttf-parser/fontdb).
pub fn alias_font_family(raw: &[u8], family: &str) -> Vec<u8> {
    match parse_sfnt(raw) {
        Some((flavor, mut entries)) => {
            entries.retain(|(tag, _)| tag != b"name");
            let name = build_name_table(family);
            entries.push((*b"name", name));
            // sfnt directories are sorted by tag.
            entries.sort_by_key(|e| e.0);
            assemble_sfnt(flavor, &entries)
        }
        None => raw.to_vec(),
    }
}

type Entry = ([u8; 4], Vec<u8>);

/// Split a raw sfnt into (flavor, [(tag, table_data)]).
fn parse_sfnt(data: &[u8]) -> Option<(u32, Vec<Entry>)> {
    if data.len() < 12 {
        return None;
    }
    let be32 = |o: usize| u32::from_be_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]);
    let flavor = be32(0);
    let num = be32(4) as usize;
    if num == 0 || num > 512 || data.len() < 12 + 16 * num {
        return None;
    }
    let mut out = Vec::with_capacity(num);
    for i in 0..num {
        let base = 12 + 16 * i;
        let mut tag = [0u8; 4];
        tag.copy_from_slice(&data[base..base + 4]);
        let offset = be32(base + 8) as usize;
        let len = be32(base + 12) as usize;
        if offset.saturating_add(len) > data.len() {
            return None;
        }
        out.push((tag, data[offset..offset + len].to_vec()));
    }
    Some((flavor, out))
}

/// Build a minimal sfnt name table (format 0) carrying the family.
fn build_name_table(family: &str) -> Vec<u8> {
    // (platformID, encodingID, languageID, nameID, bytes)
    let utf16 = |s: &str| -> Vec<u8> { s.encode_utf16().flat_map(|c| c.to_be_bytes()).collect() };
    let ascii = |s: &str| -> Vec<u8> {
        s.chars().map(|c| if c.is_ascii() { c as u8 } else { b'?' }).collect()
    };
    let ps = family.replace(' ', "");
    let records: [(u8, u8, u16, u16, Vec<u8>); 8] = [
        (1, 0, 0, 1, ascii(family)),        // Mac Roman family
        (1, 0, 0, 2, b"Regular".to_vec()),  // Mac subfamily
        (1, 0, 0, 4, ascii(family)),        // Mac full name
        (1, 0, 0, 6, ascii(&ps)),           // Mac postscript
        (3, 1, 0x409, 1, utf16(family)),    // Windows UTF-16 family
        (3, 1, 0x409, 2, utf16("Regular")), // Windows subfamily
        (3, 1, 0x409, 4, utf16(family)),    // Windows full name
        (3, 1, 0x409, 6, ascii(&ps)),       // Windows postscript (ASCII)
    ];
    let count = records.len() as u16;
    // Header: format(2) count(2) stringOffset(2) = 6; records 12 bytes each.
    let string_offset = 6 + 12 * records.len();
    let mut header: Vec<u8> = Vec::new();
    header.extend_from_slice(&0u16.to_be_bytes());
    header.extend_from_slice(&count.to_be_bytes());
    header.extend_from_slice(&(string_offset.to_be_bytes()));
    let mut strings: Vec<u8> = Vec::new();
    for (p, e, l, id, data) in &records {
        header.extend_from_slice(&(*p as u16).to_be_bytes());
        header.extend_from_slice(&(*e as u16).to_be_bytes());
        header.extend_from_slice(&l.to_be_bytes());
        header.extend_from_slice(&id.to_be_bytes());
        header.extend_from_slice(&(data.len() as u16).to_be_bytes());
        header.extend_from_slice(&(strings.len() as u16).to_be_bytes());
        strings.extend_from_slice(data);
    }
    header.extend_from_slice(&strings);
    header
}

/// Reassemble an sfnt from (flavor, tables) with 4-byte alignment.
fn assemble_sfnt(flavor: u32, entries: &[Entry]) -> Vec<u8> {
    let n = entries.len();
    let dir_size = 12 + 16 * n;
    let total: usize = entries.iter().map(|(_, d)| (d.len() + 3) & !3).sum::<usize>() + dir_size;
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&flavor.to_be_bytes());
    out.extend_from_slice(&(n as u16).to_be_bytes());
    let mut entry_selector = 0u16;
    while (1u16 << (entry_selector + 1)) <= n as u16 {
        entry_selector += 1;
    }
    let search_range = (1u16 << entry_selector) * 16;
    out.extend_from_slice(&search_range.to_be_bytes());
    out.extend_from_slice(&entry_selector.to_be_bytes());
    out.extend_from_slice(&((n as u16) * 16 - search_range).to_be_bytes());
    let mut cursor = dir_size as u32;
    for (tag, data) in entries {
        let checksum = table_checksum(data);
        out.extend_from_slice(tag);
        out.extend_from_slice(&checksum.to_be_bytes());
        out.extend_from_slice(&cursor.to_be_bytes());
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        cursor += (data.len() as u32 + 3) & !3;
    }
    for (_, data) in entries {
        out.extend_from_slice(data);
        while out.len() % 4 != 0 {
            out.push(0);
        }
    }
    out
}

/// sfnt table checksum (sum of big-endian u32s, padded with zeros).
fn table_checksum(data: &[u8]) -> u32 {
    let mut sum = 0u32;
    let mut chunks = data.chunks_exact(4);
    for c in &mut chunks {
        sum = sum.wrapping_add(u32::from_be_bytes([c[0], c[1], c[2], c[3]]));
    }
    let rem = chunks.remainder();
    if !rem.is_empty() {
        let mut last = [0u8; 4];
        last[..rem.len()].copy_from_slice(rem);
        sum = sum.wrapping_add(u32::from_be_bytes(last));
    }
    sum
}
