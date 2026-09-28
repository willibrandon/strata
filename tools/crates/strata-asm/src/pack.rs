//! Boot images (`docs/toolchain.md`, "Boot image format").
//!
//! ```text
//! 0x00 magic   0x41525453 ("STRA")
//! 0x04 version 1
//! 0x08 entry
//! 0x0C nseg    1..=15
//! 0x10 hcrc    CRC-32 of bytes [0, 0x14 + 12*nseg) with this field zero
//! 0x14 + 12*i  seg[i].addr, seg[i].len, seg[i].crc
//! payloads, in order, without padding
//! ```

/// `"STRA"` as a little-endian word.
pub const MAGIC: u32 = 0x4152_5453;
/// Format version.
pub const VERSION: u32 = 1;
/// Maximum number of segments.
pub const MAX_SEGMENTS: usize = 15;
/// Offset of the segment table.
pub const TABLE_OFFSET: usize = 0x14;

const CRC_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
};

/// IEEE 802.3 CRC-32 (reflected polynomial `0xEDB88320`, initial value and
/// final XOR `0xFFFFFFFF`).
pub fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c = CRC_TABLE[((c ^ u32::from(b)) & 0xFF) as usize] ^ (c >> 8);
    }
    c ^ 0xFFFF_FFFF
}

/// One segment: bytes to copy to `addr`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    /// Load address (4-byte aligned).
    pub addr: u32,
    /// Payload (a multiple of 4 bytes).
    pub data: Vec<u8>,
}

/// A decoded boot image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    /// Address the loader jumps to.
    pub entry: u32,
    /// Segments in image order.
    pub segments: Vec<Segment>,
}

/// Pads `data` to a multiple of 4 bytes and removes trailing all-zero words
/// (the loader does not clear memory; programs clear their own `.bss`).
pub fn trim_segment(data: &[u8]) -> Vec<u8> {
    let mut v = data.to_vec();
    while v.len() % 4 != 0 {
        v.push(0);
    }
    while v.len() >= 4 && v[v.len() - 4..].iter().all(|&b| b == 0) {
        v.truncate(v.len() - 4);
    }
    v
}

fn put(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn get(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

/// Builds an image. Segments must be 4-byte aligned, a multiple of 4 bytes
/// long, inside the 32-bit address space and non-overlapping.
pub fn build_image(entry: u32, segments: &[Segment]) -> Result<Vec<u8>, String> {
    if segments.is_empty() || segments.len() > MAX_SEGMENTS {
        return Err(format!(
            "an image needs 1 to {MAX_SEGMENTS} segments, not {}",
            segments.len()
        ));
    }
    let mut spans = Vec::new();
    for (i, s) in segments.iter().enumerate() {
        if s.addr % 4 != 0 {
            return Err(format!(
                "segment {i} address {:#010x} is not 4-byte aligned",
                s.addr
            ));
        }
        if s.data.len() % 4 != 0 {
            return Err(format!(
                "segment {i} length {} is not a multiple of 4",
                s.data.len()
            ));
        }
        let end = u64::from(s.addr) + s.data.len() as u64;
        if end > 1 << 32 {
            return Err(format!(
                "segment {i} at {:#010x} extends beyond the address space",
                s.addr
            ));
        }
        spans.push((u64::from(s.addr), end, i));
    }
    spans.sort();
    for w in spans.windows(2) {
        if w[0].1 > w[1].0 {
            return Err(format!("segments {} and {} overlap", w[0].2, w[1].2));
        }
    }
    let mut out = Vec::new();
    put(&mut out, MAGIC);
    put(&mut out, VERSION);
    put(&mut out, entry);
    put(&mut out, segments.len() as u32);
    put(&mut out, 0);
    for s in segments {
        put(&mut out, s.addr);
        put(&mut out, s.data.len() as u32);
        put(&mut out, crc32(&s.data));
    }
    let hcrc = crc32(&out);
    out[0x10..0x14].copy_from_slice(&hcrc.to_le_bytes());
    for s in segments {
        out.extend_from_slice(&s.data);
    }
    Ok(out)
}

/// Parses and fully validates an image (magic, version, segment count,
/// header CRC, lengths and segment CRCs).
pub fn parse_image(b: &[u8]) -> Result<Image, String> {
    if b.len() < TABLE_OFFSET {
        return Err("image is shorter than its header".to_string());
    }
    if get(b, 0) != MAGIC {
        return Err(format!("bad magic {:#010x}", get(b, 0)));
    }
    if get(b, 4) != VERSION {
        return Err(format!("unsupported version {}", get(b, 4)));
    }
    let entry = get(b, 8);
    let nseg = get(b, 12) as usize;
    if nseg == 0 || nseg > MAX_SEGMENTS {
        return Err(format!("invalid segment count {nseg}"));
    }
    let table_end = TABLE_OFFSET + 12 * nseg;
    if b.len() < table_end {
        return Err("image is shorter than its segment table".to_string());
    }
    let mut header = b[..table_end].to_vec();
    header[0x10..0x14].fill(0);
    let hcrc = get(b, 0x10);
    if crc32(&header) != hcrc {
        return Err(format!(
            "header CRC mismatch (stored {hcrc:#010x}, computed {:#010x})",
            crc32(&header)
        ));
    }
    let mut segments = Vec::new();
    let mut at = table_end;
    for i in 0..nseg {
        let e = TABLE_OFFSET + 12 * i;
        let (addr, len, crc) = (get(b, e), get(b, e + 4) as usize, get(b, e + 8));
        if addr % 4 != 0 || len % 4 != 0 {
            return Err(format!("segment {i} is not word aligned"));
        }
        let data = b
            .get(at..at + len)
            .ok_or(format!("segment {i} payload is truncated"))?;
        if crc32(data) != crc {
            return Err(format!("segment {i} CRC mismatch"));
        }
        segments.push(Segment {
            addr,
            data: data.to_vec(),
        });
        at += len;
    }
    if at != b.len() {
        return Err(format!(
            "{} unexpected bytes after the last segment",
            b.len() - at
        ));
    }
    Ok(Image { entry, segments })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(b"a"), 0xE8B7_BE43);
    }

    #[test]
    fn round_trip() {
        let segs = vec![
            Segment {
                addr: 0x1000_0000,
                data: vec![0x13, 0, 0, 0, 1, 2, 3, 4],
            },
            Segment {
                addr: 0x2000_0000,
                data: vec![],
            },
            Segment {
                addr: 0x2001_0000,
                data: (0..64u8).collect(),
            },
        ];
        let img = build_image(0x1000_0000, &segs).unwrap();
        assert_eq!(get(&img, 0), MAGIC);
        assert_eq!(&img[0..4], b"STRA");
        assert_eq!(img.len(), TABLE_OFFSET + 36 + 8 + 64);
        let parsed = parse_image(&img).unwrap();
        assert_eq!(
            parsed,
            Image {
                entry: 0x1000_0000,
                segments: segs
            }
        );
    }

    #[test]
    fn detects_corruption() {
        let img = build_image(
            4,
            &[Segment {
                addr: 0,
                data: vec![1, 2, 3, 4],
            }],
        )
        .unwrap();
        for i in 0..img.len() {
            let mut bad = img.clone();
            bad[i] ^= 0x40;
            assert!(parse_image(&bad).is_err(), "flip at byte {i} not detected");
        }
        assert!(parse_image(&img[..img.len() - 1]).is_err());
        let mut long = img.clone();
        long.push(0);
        assert!(parse_image(&long).is_err());
    }

    #[test]
    fn validation() {
        let s = |addr, n| Segment {
            addr,
            data: vec![0xAA; n],
        };
        assert!(build_image(0, &[]).is_err());
        assert!(build_image(0, &vec![s(0, 4); 16]).is_err());
        assert!(build_image(0, &[s(2, 4)]).is_err());
        assert!(build_image(0, &[s(0, 3)]).is_err());
        assert!(build_image(0, &[s(0, 8), s(4, 4)]).is_err());
        assert!(build_image(0, &[s(0xFFFF_FFFC, 8)]).is_err());
        assert!(build_image(0, &[s(0, 8), s(8, 4)]).is_ok());
    }

    #[test]
    fn trimming() {
        assert_eq!(trim_segment(&[1, 0, 0, 0, 0, 0, 0, 0, 0]), vec![1, 0, 0, 0]);
        assert_eq!(trim_segment(&[0, 0, 0, 0]), Vec::<u8>::new());
        assert_eq!(trim_segment(&[1, 2, 3]), vec![1, 2, 3, 0]);
        assert_eq!(trim_segment(&[0, 0, 0, 5, 0, 0]), vec![0, 0, 0, 5]);
    }
}
