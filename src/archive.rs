//! Plain archives — files leaving the entity system for good, as a `.zip` or a
//! `.tar` anyone can open (`DESIGN-2026-09-14-b` §6, format 2; Files F4).
//!
//! **Two formats because two audiences open them.** A `.zip` opens by
//! double-click in Windows 10 and 11 Explorer, macOS Finder and every Linux
//! desktop; a `.tar.gz` is what Unix tooling (and the VMs' `makeTar`) expects,
//! and Windows 10's Explorer cannot open one. Both writers share one path rule
//! ([`clean_path`]) and one duplicate rule ([`unique_names`]).
//!
//! **Export only.** Bringing an archive *in* is the dangerous direction and is
//! scoped separately (design §6: an import lands under `imports/{stamp}/` and
//! nowhere else) — nothing here reads an archive back into the tree.
//!
//! POSIX ustar, written by hand: ~60 lines, no dependency, and the same rule the
//! VMs' `vm-sdk.js makeTar` follows, so a folder that left a machine and a folder
//! that left the File Manager open the same way. Compression is the browser's
//! own `CompressionStream('gzip')` (`crate::ops::gzip`), not a crate.
//!
//! Two things a naive writer gets wrong, both tested:
//! - **a name longer than 100 bytes** is split at a `/` into ustar's 155-byte
//!   prefix; one that cannot be split is **skipped and reported**, never
//!   truncated into a different path;
//! - **two files with one name** (two apps each kept `notes.txt`) must not
//!   overwrite each other on extraction — [`unique_names`] suffixes the later.

/// One file to put in an archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveEntry {
    /// Relative path inside the archive, `/`-separated.
    pub path: String,
    pub bytes: Vec<u8>,
    /// Unix seconds; 0 when not known.
    pub mtime: u64,
}

/// What [`write_tar`] or [`write_zip`] produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Archive {
    pub bytes: Vec<u8>,
    pub written: usize,
    /// Paths that could not be represented in ustar and were left out.
    pub skipped: Vec<String>,
}

fn put(h: &mut [u8], off: usize, len: usize, s: &[u8]) {
    let n = s.len().min(len);
    h[off..off + n].copy_from_slice(&s[..n]);
}

fn octal(n: u64, len: usize) -> Vec<u8> {
    let mut s = format!("{:0width$o}", n, width = len - 1).into_bytes();
    s.push(0);
    s
}

/// Split a path into ustar's `(prefix, name)`, or `None` when it cannot fit.
fn split_path(path: &str) -> Option<(&str, &str)> {
    if path.len() <= 100 {
        return Some(("", path));
    }
    // The LAST `/` that leaves a name of at most 100 bytes and a prefix of at
    // most 155.
    path.match_indices('/').rev().map(|(i, _)| (&path[..i], &path[i + 1..])).find(|(pre, name)| {
        !name.is_empty() && name.len() <= 100 && pre.len() <= 155
    })
}

/// A path safe to extract: relative, no `.`/`..` segments, no empty segments.
fn clean_path(path: &str) -> Option<String> {
    let parts: Vec<&str> = path.split(['/', '\\']).filter(|p| !p.is_empty() && *p != ".").collect();
    if parts.is_empty() || parts.iter().any(|p| *p == ".." || p.contains('\0')) {
        return None;
    }
    Some(parts.join("/"))
}

pub fn write_tar(entries: &[ArchiveEntry]) -> Archive {
    let mut out = Vec::new();
    let mut skipped = Vec::new();
    let mut written = 0;
    for e in entries {
        let Some(path) = clean_path(&e.path) else {
            skipped.push(e.path.clone());
            continue;
        };
        let Some((pre, name)) = split_path(&path) else {
            skipped.push(e.path.clone());
            continue;
        };
        let mut h = [0u8; 512];
        put(&mut h, 0, 100, name.as_bytes());
        put(&mut h, 100, 8, &octal(0o644, 8));
        put(&mut h, 108, 8, &octal(0, 8));
        put(&mut h, 116, 8, &octal(0, 8));
        put(&mut h, 124, 12, &octal(e.bytes.len() as u64, 12));
        put(&mut h, 136, 12, &octal(e.mtime, 12));
        put(&mut h, 148, 8, b"        ");
        h[156] = b'0';
        put(&mut h, 257, 6, b"ustar\0");
        put(&mut h, 263, 2, b"00");
        put(&mut h, 345, 155, pre.as_bytes());
        let sum: u32 = h.iter().map(|b| *b as u32).sum();
        let mut chk = format!("{:06o}", sum).into_bytes();
        chk.extend_from_slice(b"\0 ");
        put(&mut h, 148, 8, &chk);
        out.extend_from_slice(&h);
        out.extend_from_slice(&e.bytes);
        out.resize(out.len() + (512 - e.bytes.len() % 512) % 512, 0);
        written += 1;
    }
    out.resize(out.len() + 1024, 0);
    Archive { bytes: out, written, skipped }
}

/// CRC-32 (IEEE 802.3, reflected, poly `0xEDB88320`) — the checksum zip stores
/// per member. Bitwise rather than a table: archives are built once per click.
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for b in bytes {
        c ^= *b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 };
        }
    }
    !c
}

/// Unix seconds → MS-DOS `(time, date)`, UTC. Zip cannot say a year before
/// 1980, so an unknown time (0) or an earlier one reads as 1980-01-01 00:00.
fn dos_time(unix: u64) -> (u16, u16) {
    const Y1980: u64 = 315_532_800;
    if unix < Y1980 {
        return (0, (1 << 5) | 1);
    }
    let days = (unix / 86_400) as i64;
    let rem = unix % 86_400;
    // Howard Hinnant's days-to-civil.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    let year = (y - 1980).clamp(0, 127) as u16;
    let (h, min, sec) = ((rem / 3600) as u16, ((rem % 3600) / 60) as u16, (rem % 60) as u16);
    ((h << 11) | (min << 5) | (sec / 2), (year << 9) | ((m as u16) << 5) | d as u16)
}

/// Write a `.zip`. `deflated[i]`, when present, is entry `i`'s bytes already
/// compressed as **raw DEFLATE** (RFC 1951, the browser's
/// `CompressionStream('deflate-raw')`); it is used only when it is smaller, so
/// an engine that cannot compress — or a file that does not shrink — is stored.
/// Compression happens outside because it is async in the browser and this
/// writer is pure.
///
/// Not written, and refused per entry rather than truncated: a member over
/// 4 GiB or an archive past 4 GiB (that needs Zip64, and a tab holding the
/// whole archive in memory will not get there). Names are UTF-8 and flagged so
/// (general-purpose bit 11), which Windows 10+ Explorer, macOS and Info-ZIP read.
pub fn write_zip(entries: &[ArchiveEntry], deflated: &[Option<Vec<u8>>]) -> Archive {
    let mut out: Vec<u8> = Vec::new();
    let mut central: Vec<u8> = Vec::new();
    let mut skipped = Vec::new();
    let mut written: usize = 0;
    let le16 = |v: &mut Vec<u8>, n: u16| v.extend_from_slice(&n.to_le_bytes());
    let le32 = |v: &mut Vec<u8>, n: u32| v.extend_from_slice(&n.to_le_bytes());
    for (i, e) in entries.iter().enumerate() {
        let Some(path) = clean_path(&e.path) else {
            skipped.push(e.path.clone());
            continue;
        };
        let (method, data): (u16, &[u8]) = match deflated.get(i).and_then(|d| d.as_deref()) {
            Some(d) if d.len() < e.bytes.len() => (8, d),
            _ => (0, &e.bytes),
        };
        let offset = out.len();
        if path.len() > u16::MAX as usize
            || e.bytes.len() > u32::MAX as usize
            || offset + 30 + path.len() + data.len() > u32::MAX as usize
        {
            skipped.push(e.path.clone());
            continue;
        }
        let crc = crc32(&e.bytes);
        let (time, date) = dos_time(e.mtime);
        // Local file header.
        le32(&mut out, 0x0403_4b50);
        le16(&mut out, 20); // version needed: 2.0 (deflate)
        le16(&mut out, 1 << 11); // UTF-8 names
        le16(&mut out, method);
        le16(&mut out, time);
        le16(&mut out, date);
        le32(&mut out, crc);
        le32(&mut out, data.len() as u32);
        le32(&mut out, e.bytes.len() as u32);
        le16(&mut out, path.len() as u16);
        le16(&mut out, 0);
        out.extend_from_slice(path.as_bytes());
        out.extend_from_slice(data);
        // Central directory record.
        le32(&mut central, 0x0201_4b50);
        le16(&mut central, (3 << 8) | 20); // made by: Unix, 2.0 — so the mode below is read
        le16(&mut central, 20);
        le16(&mut central, 1 << 11);
        le16(&mut central, method);
        le16(&mut central, time);
        le16(&mut central, date);
        le32(&mut central, crc);
        le32(&mut central, data.len() as u32);
        le32(&mut central, e.bytes.len() as u32);
        le16(&mut central, path.len() as u16);
        le16(&mut central, 0); // extra
        le16(&mut central, 0); // comment
        le16(&mut central, 0); // disk
        le16(&mut central, 0); // internal attrs
        le32(&mut central, 0o100644 << 16); // a regular file, rw-r--r--
        le32(&mut central, offset as u32);
        central.extend_from_slice(path.as_bytes());
        written += 1;
    }
    let cd_offset = out.len();
    if written > u16::MAX as usize || cd_offset + central.len() > u32::MAX as usize {
        // Past what a plain zip can index: hand back nothing rather than an
        // archive whose directory lies about what it holds.
        return Archive { bytes: Vec::new(), written: 0, skipped: entries.iter().map(|e| e.path.clone()).collect() };
    }
    out.extend_from_slice(&central);
    le32(&mut out, 0x0605_4b50);
    le16(&mut out, 0);
    le16(&mut out, 0);
    le16(&mut out, written as u16);
    le16(&mut out, written as u16);
    le32(&mut out, central.len() as u32);
    le32(&mut out, cd_offset as u32);
    le16(&mut out, 0);
    Archive { bytes: out, written, skipped }
}

/// Give every entry a distinct path: a later `notes.txt` becomes
/// `notes (2).txt`, keeping the extension a person's system opens it with.
pub fn unique_names(entries: &mut [ArchiveEntry]) {
    let mut seen = std::collections::HashSet::new();
    for e in entries.iter_mut() {
        if seen.insert(e.path.clone()) {
            continue;
        }
        let (stem, ext) = match e.path.rfind('.') {
            Some(i) if i > 0 && !e.path[i..].contains('/') => (e.path[..i].to_string(), e.path[i..].to_string()),
            _ => (e.path.clone(), String::new()),
        };
        let mut n = 2;
        loop {
            let candidate = format!("{stem} ({n}){ext}");
            if seen.insert(candidate.clone()) {
                e.path = candidate;
                break;
            }
            n += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Read a ustar back — the test's own reader, written to the format, not
    /// to the writer above.
    fn read_tar(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
        let mut out = Vec::new();
        let mut off = 0;
        while off + 512 <= bytes.len() {
            let h = &bytes[off..off + 512];
            if h.iter().all(|b| *b == 0) {
                break;
            }
            let text = |a: usize, b: usize| {
                let s = &h[a..b];
                String::from_utf8(s[..s.iter().position(|c| *c == 0).unwrap_or(s.len())].to_vec()).unwrap()
            };
            assert_eq!(&h[257..263], b"ustar\0", "magic");
            let stored: u32 = u32::from_str_radix(text(148, 154).trim(), 8).unwrap();
            let mut copy = h.to_vec();
            copy[148..156].copy_from_slice(b"        ");
            assert_eq!(stored, copy.iter().map(|b| *b as u32).sum::<u32>(), "checksum");
            let size = usize::from_str_radix(text(124, 135).trim(), 8).unwrap();
            let (pre, name) = (text(345, 500), text(0, 100));
            let path = if pre.is_empty() { name } else { format!("{pre}/{name}") };
            off += 512;
            out.push((path, bytes[off..off + size].to_vec()));
            off += size.div_ceil(512) * 512;
        }
        out
    }

    /// Read a zip back through its central directory — the test's own reader,
    /// written to APPNOTE, checking each local header agrees with it.
    fn read_zip(bytes: &[u8]) -> Vec<(String, u16, u32, Vec<u8>)> {
        let u16at = |o: usize| u16::from_le_bytes([bytes[o], bytes[o + 1]]);
        let u32at = |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
        let eocd = bytes.len() - 22;
        assert_eq!(u32at(eocd), 0x0605_4b50, "end of central directory");
        let count = u16at(eocd + 10) as usize;
        let mut cd = u32at(eocd + 16) as usize;
        assert_eq!(cd + u32at(eocd + 12) as usize, eocd, "the directory ends where the end record starts");
        let mut out = Vec::new();
        for _ in 0..count {
            assert_eq!(u32at(cd), 0x0201_4b50);
            let (flags, method, crc) = (u16at(cd + 8), u16at(cd + 10), u32at(cd + 16));
            let (csize, usize_, nlen) = (u32at(cd + 20) as usize, u32at(cd + 24), u16at(cd + 28) as usize);
            let skip = nlen + u16at(cd + 30) as usize + u16at(cd + 32) as usize;
            let local = u32at(cd + 42) as usize;
            let name = String::from_utf8(bytes[cd + 46..cd + 46 + nlen].to_vec()).unwrap();
            assert_eq!(flags & (1 << 11), 1 << 11, "UTF-8 flag");
            assert_eq!(u32at(local), 0x0403_4b50);
            assert_eq!((u16at(local + 8), u32at(local + 14)), (method, crc), "local header disagrees with the directory");
            let data_at = local + 30 + u16at(local + 26) as usize + u16at(local + 28) as usize;
            assert_eq!(usize_ as usize, if method == 0 { csize } else { usize_ as usize });
            out.push((name, method, crc, bytes[data_at..data_at + csize].to_vec()));
            cd += 46 + skip;
        }
        out
    }

    #[test]
    fn crc32_matches_the_standard_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn dos_time_is_utc_and_clamps_before_1980() {
        // 2026-09-14 13:45:30 UTC (from `date -u -d @1789393530`).
        let (t, d) = dos_time(1_789_393_530);
        assert_eq!((d >> 9, (d >> 5) & 0xF, d & 0x1F), (46, 9, 14));
        assert_eq!((t >> 11, (t >> 5) & 0x3F, (t & 0x1F) * 2), (13, 45, 30));
        assert_eq!(dos_time(0), (0, (1 << 5) | 1));
        // Leap day: 2024-02-29 00:00:00 UTC.
        let (_, d) = dos_time(1_709_164_800);
        assert_eq!((d >> 9, (d >> 5) & 0xF, d & 0x1F), (44, 2, 29));
    }

    #[test]
    fn files_round_trip_through_a_zip_stored_or_deflated() {
        let entries = vec![
            ArchiveEntry { path: "notes.txt".into(), bytes: b"hello hello hello".to_vec(), mtime: 1_700_000_000 },
            ArchiveEntry { path: "empty".into(), bytes: vec![], mtime: 0 },
            ArchiveEntry { path: "dir/café.bin".into(), bytes: vec![7u8; 600], mtime: 0 },
        ];
        // Stand-in "compressed" bytes: the writer never inflates, so any shorter
        // run proves the method switch; a longer one proves it is not taken.
        let deflated = vec![Some(b"short".to_vec()), None, Some(vec![0u8; 700])];
        let zip = write_zip(&entries, &deflated);
        assert_eq!((zip.written, zip.skipped.len()), (3, 0));
        let back = read_zip(&zip.bytes);
        assert_eq!(back.iter().map(|m| (m.0.as_str(), m.1)).collect::<Vec<_>>(),
            vec![("notes.txt", 8), ("empty", 0), ("dir/café.bin", 0)]);
        assert_eq!(back[0].3, b"short", "a smaller deflate is what is stored");
        assert_eq!(back[2].3, vec![7u8; 600], "a deflate that does not shrink is not used");
        for (m, e) in back.iter().zip(&entries) {
            assert_eq!(m.2, crc32(&e.bytes), "the CRC is of the original bytes, not the compressed ones");
        }
    }

    #[test]
    fn a_zip_refuses_the_same_escaping_paths_the_tar_does() {
        let zip = write_zip(
            &[
                ArchiveEntry { path: "../../etc/passwd".into(), bytes: b"x".to_vec(), mtime: 0 },
                ArchiveEntry { path: "/abs/ok.txt".into(), bytes: b"y".to_vec(), mtime: 0 },
                ArchiveEntry { path: "C:\\evil\\..\\x".into(), bytes: b"z".to_vec(), mtime: 0 },
            ],
            &[],
        );
        assert_eq!(zip.skipped, vec!["../../etc/passwd".to_string(), "C:\\evil\\..\\x".to_string()]);
        assert_eq!(read_zip(&zip.bytes).into_iter().map(|m| m.0).collect::<Vec<_>>(), vec!["abs/ok.txt"]);
    }

    #[test]
    fn an_empty_zip_is_still_a_zip() {
        let zip = write_zip(&[], &[]);
        assert_eq!(zip.bytes.len(), 22);
        assert!(read_zip(&zip.bytes).is_empty());
    }

    #[test]
    fn files_round_trip_through_the_archive() {
        let entries = vec![
            ArchiveEntry { path: "notes.txt".into(), bytes: b"hello".to_vec(), mtime: 1_700_000_000 },
            ArchiveEntry { path: "empty".into(), bytes: vec![], mtime: 0 },
            ArchiveEntry { path: "dir/block.bin".into(), bytes: vec![7u8; 512], mtime: 0 },
            ArchiveEntry { path: "dir/odd.bin".into(), bytes: vec![1u8; 513], mtime: 0 },
        ];
        let tar = write_tar(&entries);
        assert_eq!(tar.bytes.len() % 512, 0);
        assert_eq!((tar.written, tar.skipped.len()), (4, 0));
        let back = read_tar(&tar.bytes);
        assert_eq!(back, entries.iter().map(|e| (e.path.clone(), e.bytes.clone())).collect::<Vec<_>>());
    }

    #[test]
    fn a_long_path_is_split_and_one_that_cannot_be_is_skipped_not_truncated() {
        let long_dir = "d".repeat(120);
        let ok = format!("{long_dir}/file.txt");
        let unsplittable = "x".repeat(150);
        let tar = write_tar(&[
            ArchiveEntry { path: ok.clone(), bytes: b"a".to_vec(), mtime: 0 },
            ArchiveEntry { path: unsplittable.clone(), bytes: b"b".to_vec(), mtime: 0 },
        ]);
        assert_eq!(tar.skipped, vec![unsplittable]);
        assert_eq!(read_tar(&tar.bytes), vec![(ok, b"a".to_vec())]);
    }

    #[test]
    fn a_path_that_would_escape_the_folder_it_is_extracted_into_is_refused() {
        let tar = write_tar(&[
            ArchiveEntry { path: "../../etc/passwd".into(), bytes: b"x".to_vec(), mtime: 0 },
            ArchiveEntry { path: "/abs/ok.txt".into(), bytes: b"y".to_vec(), mtime: 0 },
            ArchiveEntry { path: "./a//b.txt".into(), bytes: b"z".to_vec(), mtime: 0 },
        ]);
        assert_eq!(tar.skipped, vec!["../../etc/passwd".to_string()]);
        let names: Vec<_> = read_tar(&tar.bytes).into_iter().map(|(p, _)| p).collect();
        assert_eq!(names, vec!["abs/ok.txt", "a/b.txt"]);
    }

    #[test]
    fn two_files_with_one_name_both_survive_extraction() {
        let mut entries: Vec<ArchiveEntry> = ["notes.txt", "notes.txt", "notes (2).txt", "README", "README"]
            .iter()
            .map(|p| ArchiveEntry { path: p.to_string(), bytes: vec![], mtime: 0 })
            .collect();
        unique_names(&mut entries);
        let names: Vec<_> = entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(names, vec!["notes.txt", "notes (2).txt", "notes (2) (2).txt", "README", "README (2)"]);
    }
}
