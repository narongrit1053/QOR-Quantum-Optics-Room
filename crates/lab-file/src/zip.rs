//! The zip subset LabSetup needs: a deterministic writer (store or deflate, no zip64) and a
//! strict, bounded reader that takes sizes from the central directory, so archives written by
//! Explorer or 7-Zip (data descriptors) load too.

use std::io::{Read, Write};

use flate2::Crc;
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;

const LOCAL_SIG: u32 = 0x0403_4b50;
const CENTRAL_SIG: u32 = 0x0201_4b50;
const EOCD_SIG: u32 = 0x0605_4b50;
const EOCD_LEN: usize = 22;
const LOCAL_LEN: usize = 30;
const CENTRAL_LEN: usize = 46;
/// Deflate cannot expand data by more than about 1032:1; bounds the up-front allocation.
const MAX_INFLATE_RATIO: usize = 1032;

pub(crate) struct ZipEntryOut<'a> {
    pub name: &'a str,
    pub data: &'a [u8],
}

#[derive(Debug, PartialEq)]
pub(crate) enum ZipError {
    NotZip,
    Truncated,
    Unsupported(&'static str),
    BadCrc,
    BadDeflate,
    TooLarge,
    Missing,
    Duplicate,
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = Crc::new();
    crc.update(data);
    crc.sum()
}

fn deflate(data: &[u8]) -> Option<Vec<u8>> {
    let mut encoder = DeflateEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(data).ok()?;
    encoder.finish().ok()
}

fn put16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

/// Entries must be smaller than 4 GiB with names under 64 KiB (the caller enforces its own,
/// much lower, limits). The output depends only on the input.
pub(crate) fn write_zip(entries: &[ZipEntryOut]) -> Vec<u8> {
    struct Done {
        offset: u32,
        crc: u32,
        method: u16,
        compressed: u32,
        size: u32,
    }
    let mut out = Vec::new();
    let mut done = Vec::with_capacity(entries.len());
    for entry in entries {
        let data = entry.data;
        let (method, body) = match deflate(data) {
            Some(packed) if packed.len() < data.len() => (8u16, packed),
            _ => (0u16, data.to_vec()),
        };
        let item = Done {
            offset: out.len() as u32,
            crc: crc32(data),
            method,
            compressed: body.len() as u32,
            size: data.len() as u32,
        };
        put32(&mut out, LOCAL_SIG);
        put16(&mut out, 20);
        put16(&mut out, 0x0800);
        put16(&mut out, item.method);
        put16(&mut out, 0x0000);
        put16(&mut out, 0x0021);
        put32(&mut out, item.crc);
        put32(&mut out, item.compressed);
        put32(&mut out, item.size);
        put16(&mut out, entry.name.len() as u16);
        put16(&mut out, 0);
        out.extend_from_slice(entry.name.as_bytes());
        out.extend_from_slice(&body);
        done.push(item);
    }
    let cd_offset = out.len() as u32;
    for (entry, item) in entries.iter().zip(&done) {
        put32(&mut out, CENTRAL_SIG);
        put16(&mut out, 20);
        put16(&mut out, 20);
        put16(&mut out, 0x0800);
        put16(&mut out, item.method);
        put16(&mut out, 0x0000);
        put16(&mut out, 0x0021);
        put32(&mut out, item.crc);
        put32(&mut out, item.compressed);
        put32(&mut out, item.size);
        put16(&mut out, entry.name.len() as u16);
        put16(&mut out, 0);
        put16(&mut out, 0);
        put16(&mut out, 0);
        put16(&mut out, 0);
        put32(&mut out, 0);
        put32(&mut out, item.offset);
        out.extend_from_slice(entry.name.as_bytes());
    }
    let cd_size = out.len() as u32 - cd_offset;
    put32(&mut out, EOCD_SIG);
    put16(&mut out, 0);
    put16(&mut out, 0);
    put16(&mut out, entries.len() as u16);
    put16(&mut out, entries.len() as u16);
    put32(&mut out, cd_size);
    put32(&mut out, cd_offset);
    put16(&mut out, 0);
    out
}

fn u16_at(bytes: &[u8], at: usize) -> Result<u16, ZipError> {
    let end = at.checked_add(2).ok_or(ZipError::Truncated)?;
    let slice = bytes.get(at..end).ok_or(ZipError::Truncated)?;
    Ok(u16::from_le_bytes([slice[0], slice[1]]))
}

fn u32_at(bytes: &[u8], at: usize) -> Result<u32, ZipError> {
    let end = at.checked_add(4).ok_or(ZipError::Truncated)?;
    let slice = bytes.get(at..end).ok_or(ZipError::Truncated)?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

/// What the file looks like when no end-of-central-directory record can be found.
fn no_eocd(archive: &[u8]) -> ZipError {
    if u32_at(archive, 0) == Ok(LOCAL_SIG) { ZipError::Truncated } else { ZipError::NotZip }
}

struct Entry {
    flags: u16,
    method: u16,
    crc: u32,
    compressed: u32,
    size: u32,
    local_offset: u32,
}

/// The single central-directory entry called `name`.
fn find_entry(archive: &[u8], name: &str) -> Result<Entry, ZipError> {
    if archive.len() < EOCD_LEN {
        return Err(no_eocd(archive));
    }
    let last = archive.len() - EOCD_LEN;
    let first = last.saturating_sub(usize::from(u16::MAX));
    let eocd = (first..=last)
        .rev()
        .find(|&pos| {
            u32_at(archive, pos) == Ok(EOCD_SIG)
                && u16_at(archive, pos + 20).is_ok_and(|comment| pos + EOCD_LEN + usize::from(comment) == archive.len())
        })
        .ok_or_else(|| no_eocd(archive))?;

    let disk = u16_at(archive, eocd + 4)?;
    let cd_disk = u16_at(archive, eocd + 6)?;
    let on_disk = u16_at(archive, eocd + 8)?;
    let total = u16_at(archive, eocd + 10)?;
    let cd_size = u32_at(archive, eocd + 12)?;
    let cd_offset = u32_at(archive, eocd + 16)?;
    if disk != 0 || cd_disk != 0 || on_disk != total {
        return Err(ZipError::Unsupported("multi-disk archive"));
    }
    if total == u16::MAX || cd_size == u32::MAX || cd_offset == u32::MAX {
        return Err(ZipError::Unsupported("zip64"));
    }
    let cd_end = (cd_offset as usize).checked_add(cd_size as usize).ok_or(ZipError::Truncated)?;
    if cd_end > eocd {
        return Err(ZipError::Truncated);
    }

    let mut found = None;
    let mut at = cd_offset as usize;
    for _ in 0..total {
        if u32_at(archive, at)? != CENTRAL_SIG {
            return Err(ZipError::Truncated);
        }
        let name_len = usize::from(u16_at(archive, at + 28)?);
        let extra_len = usize::from(u16_at(archive, at + 30)?);
        let comment_len = usize::from(u16_at(archive, at + 32)?);
        let name_start = at.checked_add(CENTRAL_LEN).ok_or(ZipError::Truncated)?;
        let name_end = name_start.checked_add(name_len).ok_or(ZipError::Truncated)?;
        let next =
            name_end.checked_add(extra_len).and_then(|v| v.checked_add(comment_len)).ok_or(ZipError::Truncated)?;
        if next > cd_end {
            return Err(ZipError::Truncated);
        }
        if archive.get(name_start..name_end) == Some(name.as_bytes()) {
            if found.is_some() {
                return Err(ZipError::Duplicate);
            }
            found = Some(Entry {
                flags: u16_at(archive, at + 8)?,
                method: u16_at(archive, at + 10)?,
                crc: u32_at(archive, at + 16)?,
                compressed: u32_at(archive, at + 20)?,
                size: u32_at(archive, at + 24)?,
                local_offset: u32_at(archive, at + 42)?,
            });
        }
        at = next;
    }
    found.ok_or(ZipError::Missing)
}

/// The inflated size the central directory declares for `name`, if the entry exists.
pub(crate) fn declared_size(archive: &[u8], name: &str) -> Option<u64> {
    find_entry(archive, name).ok().map(|e| u64::from(e.size))
}

/// Reads the entry called `name`, refusing anything larger than `max_size` before allocating.
pub(crate) fn read_entry(archive: &[u8], name: &str, max_size: usize) -> Result<Vec<u8>, ZipError> {
    let entry = find_entry(archive, name)?;
    if entry.flags & 1 != 0 {
        return Err(ZipError::Unsupported("encrypted entry"));
    }
    if entry.method != 0 && entry.method != 8 {
        return Err(ZipError::Unsupported("compression method"));
    }
    if entry.compressed == u32::MAX || entry.size == u32::MAX || entry.local_offset == u32::MAX {
        return Err(ZipError::Unsupported("zip64"));
    }
    let size = entry.size as usize;
    if size > max_size {
        return Err(ZipError::TooLarge);
    }

    let local = entry.local_offset as usize;
    if u32_at(archive, local)? != LOCAL_SIG {
        return Err(ZipError::Truncated);
    }
    let name_len = usize::from(u16_at(archive, local + 26)?);
    let extra_len = usize::from(u16_at(archive, local + 28)?);
    let start = local
        .checked_add(LOCAL_LEN)
        .and_then(|v| v.checked_add(name_len))
        .and_then(|v| v.checked_add(extra_len))
        .ok_or(ZipError::Truncated)?;
    let end = start.checked_add(entry.compressed as usize).ok_or(ZipError::Truncated)?;
    let body = archive.get(start..end).ok_or(ZipError::Truncated)?;

    let data = if entry.method == 0 {
        if body.len() != size {
            return Err(ZipError::BadDeflate);
        }
        body.to_vec()
    } else {
        let mut data = Vec::with_capacity(size.min(body.len().saturating_mul(MAX_INFLATE_RATIO)));
        DeflateDecoder::new(body).take(size as u64 + 1).read_to_end(&mut data).map_err(|_| ZipError::BadDeflate)?;
        if data.len() != size {
            return Err(ZipError::BadDeflate);
        }
        data
    };
    if crc32(&data) != entry.crc {
        return Err(ZipError::BadCrc);
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn xorshift(seed: u64, len: usize) -> Vec<u8> {
        let mut s = seed;
        (0..len)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                (s >> 24) as u8
            })
            .collect()
    }

    fn one(name: &str, data: &[u8]) -> Vec<u8> {
        write_zip(&[ZipEntryOut { name, data }])
    }

    fn eocd_pos(archive: &[u8]) -> usize {
        archive.len() - EOCD_LEN
    }

    fn cd_offset(archive: &[u8]) -> usize {
        u32_at(archive, eocd_pos(archive) + 16).unwrap() as usize
    }

    /// Z1: store and deflate round trips, including the empty and the 1-byte entry.
    #[test]
    fn z1_round_trip() {
        let compressible: Vec<u8> = b"optical table ".iter().copied().cycle().take(20_000).collect();
        let incompressible = xorshift(7, 4096);
        let cases: [(&[u8], u16); 4] = [(b"", 0), (b"x", 0), (&compressible, 8), (&incompressible, 0)];
        for (data, method) in cases {
            let archive = one("project.json", data);
            assert_eq!(u16_at(&archive, 8).unwrap(), method, "method for {} bytes", data.len());
            assert_eq!(read_entry(&archive, "project.json", 1 << 20).unwrap(), data);
        }
        let two = write_zip(&[ZipEntryOut { name: "a", data: b"first" }, ZipEntryOut { name: "b", data: b"second" }]);
        assert_eq!(read_entry(&two, "b", 100).unwrap(), b"second");
        assert_eq!(read_entry(&two, "a", 100).unwrap(), b"first");
        assert_eq!(read_entry(&two, "c", 100), Err(ZipError::Missing));
    }

    /// Z2: the writer is deterministic.
    #[test]
    fn z2_deterministic() {
        let data = xorshift(3, 1000).iter().map(|b| b % 7).collect::<Vec<_>>();
        assert_eq!(one("project.json", &data), one("project.json", &data));
    }

    /// Z3: truncation anywhere is an error, never a panic.
    #[test]
    fn z3_truncation() {
        let data: Vec<u8> = b"abcdefgh".iter().copied().cycle().take(6000).collect();
        let archive = one("project.json", &data);
        for cut in (0..archive.len()).step_by(97) {
            let result = read_entry(&archive[..cut], "project.json", 1 << 20);
            assert!(matches!(result, Err(ZipError::Truncated | ZipError::NotZip)), "cut at {cut}: {result:?}");
        }
    }

    /// Z4: a flipped data byte is caught by the CRC or the inflater.
    #[test]
    fn z4_flipped_byte() {
        let data = xorshift(11, 3000);
        let mut archive = one("project.json", &data);
        archive[LOCAL_LEN + "project.json".len() + 500] ^= 0x40;
        assert!(matches!(read_entry(&archive, "project.json", 1 << 20), Err(ZipError::BadCrc | ZipError::BadDeflate)));
        let packed: Vec<u8> = b"zz".iter().copied().cycle().take(5000).collect();
        let mut archive = one("project.json", &packed);
        archive[LOCAL_LEN + "project.json".len() + 3] ^= 0xFF;
        assert!(matches!(read_entry(&archive, "project.json", 1 << 20), Err(ZipError::BadCrc | ZipError::BadDeflate)));
    }

    /// Z5: a declared 300 MiB entry is refused from the directory alone.
    #[test]
    fn z5_too_large_before_allocating() {
        let mut archive = one("project.json", b"{}");
        let at = cd_offset(&archive) + 24;
        archive[at..at + 4].copy_from_slice(&(300u32 * 1024 * 1024).to_le_bytes());
        assert_eq!(read_entry(&archive, "project.json", 256 << 20), Err(ZipError::TooLarge));
        assert_eq!(declared_size(&archive, "project.json"), Some(300 << 20));
    }

    /// Z6: encryption, an unknown method and zip64 markers are Unsupported.
    #[test]
    fn z6_unsupported() {
        let base = one("project.json", b"{\"a\":1}");
        let cd = cd_offset(&base);

        let mut encrypted = base.clone();
        encrypted[cd + 8] |= 1;
        assert!(matches!(read_entry(&encrypted, "project.json", 100), Err(ZipError::Unsupported(_))));

        let mut method = base.clone();
        method[cd + 10] = 99;
        assert!(matches!(read_entry(&method, "project.json", 100), Err(ZipError::Unsupported(_))));

        let mut zip64 = base.clone();
        let eocd = eocd_pos(&zip64);
        zip64[eocd + 10..eocd + 12].copy_from_slice(&u16::MAX.to_le_bytes());
        zip64[eocd + 8..eocd + 10].copy_from_slice(&u16::MAX.to_le_bytes());
        assert!(matches!(read_entry(&zip64, "project.json", 100), Err(ZipError::Unsupported(_))));

        let mut offset64 = base.clone();
        let eocd = eocd_pos(&offset64);
        offset64[eocd + 16..eocd + 20].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(read_entry(&offset64, "project.json", 100), Err(ZipError::Unsupported(_))));

        let mut entry64 = base;
        entry64[cd + 20..cd + 24].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(read_entry(&entry64, "project.json", 100), Err(ZipError::Unsupported(_))));
    }

    /// Z7: an archive in the Explorer/7-Zip style (flag bit 3, sizes only in the directory).
    #[test]
    fn z7_data_descriptor() {
        let name = b"project.json";
        let data: Vec<u8> = b"{\"k\": \"value\"}\n".iter().copied().cycle().take(900).collect();
        let packed = deflate(&data).unwrap();
        let crc = crc32(&data);

        let mut out = Vec::new();
        put32(&mut out, LOCAL_SIG);
        put16(&mut out, 20);
        put16(&mut out, 0x0008);
        put16(&mut out, 8);
        put16(&mut out, 0);
        put16(&mut out, 0x0021);
        put32(&mut out, 0);
        put32(&mut out, 0);
        put32(&mut out, 0);
        put16(&mut out, name.len() as u16);
        put16(&mut out, 4);
        out.extend_from_slice(name);
        out.extend_from_slice(&[0xAA; 4]);
        out.extend_from_slice(&packed);
        put32(&mut out, 0x0807_4b50);
        put32(&mut out, crc);
        put32(&mut out, packed.len() as u32);
        put32(&mut out, data.len() as u32);

        let cd = out.len();
        put32(&mut out, CENTRAL_SIG);
        put16(&mut out, 20);
        put16(&mut out, 20);
        put16(&mut out, 0x0008);
        put16(&mut out, 8);
        put16(&mut out, 0);
        put16(&mut out, 0x0021);
        put32(&mut out, crc);
        put32(&mut out, packed.len() as u32);
        put32(&mut out, data.len() as u32);
        put16(&mut out, name.len() as u16);
        put16(&mut out, 0);
        put16(&mut out, 0);
        put16(&mut out, 0);
        put16(&mut out, 0);
        put32(&mut out, 0);
        put32(&mut out, 0);
        out.extend_from_slice(name);
        let cd_size = out.len() - cd;
        put32(&mut out, EOCD_SIG);
        put16(&mut out, 0);
        put16(&mut out, 0);
        put16(&mut out, 1);
        put16(&mut out, 1);
        put32(&mut out, cd_size as u32);
        put32(&mut out, cd as u32);
        put16(&mut out, 0);

        assert_eq!(read_entry(&out, "project.json", 1 << 20).unwrap(), data);
    }

    #[test]
    fn duplicate_names_are_rejected() {
        let archive = write_zip(&[
            ZipEntryOut { name: "project.json", data: b"1" },
            ZipEntryOut { name: "project.json", data: b"2" },
        ]);
        assert_eq!(read_entry(&archive, "project.json", 10), Err(ZipError::Duplicate));
    }

    #[test]
    fn not_a_zip() {
        assert_eq!(read_entry(b"hello", "x", 10), Err(ZipError::NotZip));
        assert_eq!(read_entry(&[b'x'; 100], "x", 10), Err(ZipError::NotZip));
    }
}
