//! Bounded-memory structural probing for RPF9 archives.

use std::io::{self, Read};
use std::path::Path;

use crate::gpu::pack;

const CHUNK_SIZE: usize = 4 * 1024 * 1024;
const MAX_SHOWN_HEADERS: usize = 1024;

#[derive(Debug, PartialEq, Eq)]
enum TocEvidence {
    Encrypted,
    Different,
    Unknown,
}

#[derive(Debug)]
struct HeaderProbe {
    offset: u64,
    fields: Option<[u32; 3]>,
}

#[derive(Debug)]
struct Probe {
    root_magic: bool,
    headers: u64,
    truncated_headers: u64,
    shown: Vec<HeaderProbe>,
    toc_evidence: TocEvidence,
}

/// Scan only RPF9 header locations while retaining one fixed-size input chunk
/// and a capped number of summaries. This intentionally does not interpret,
/// decrypt, or extract the table of contents.
fn inspect(reader: &mut impl Read, len: u64, top: usize) -> io::Result<Probe> {
    let mut buffer = vec![0u8; len.min(CHUNK_SIZE as u64) as usize];
    let mut base = 0u64;
    let mut headers = 0u64;
    let mut root_magic = false;
    let mut truncated_headers = 0u64;
    let shown_limit = top.min(MAX_SHOWN_HEADERS);
    let mut shown = Vec::with_capacity(shown_limit);
    let mut first_toc: Option<[u8; 32]> = None;
    let mut complete_tocs = 0u64;
    let mut all_tocs_same = true;

    while base < len {
        let want = usize::try_from((len - base).min(CHUNK_SIZE as u64)).unwrap();
        let mut got = 0usize;
        while got < want {
            let count = reader.read(&mut buffer[got..want])?;
            if count == 0 {
                break;
            }
            got += count;
        }
        if got == 0 {
            break;
        }

        let chunk = &buffer[..got];
        for local in (0..got).step_by(0x800) {
            if chunk.get(local..local + 4) != Some(pack::RPF9_MAGIC) {
                continue;
            }
            headers += 1;
            let offset = base + local as u64;
            root_magic |= offset == 0;
            let fields = chunk.get(local + 4..local + 16).map(|bytes| {
                [
                    u32::from_le_bytes(bytes[0..4].try_into().unwrap()),
                    u32::from_le_bytes(bytes[4..8].try_into().unwrap()),
                    u32::from_le_bytes(bytes[8..12].try_into().unwrap()),
                ]
            });
            let toc = chunk.get(local + 0x40..local + 0x60).map(|bytes| {
                let mut value = [0u8; 32];
                value.copy_from_slice(bytes);
                value
            });
            if fields.is_none() || toc.is_none() {
                truncated_headers += 1;
            }
            if shown.len() < shown_limit {
                shown.push(HeaderProbe { offset, fields });
            }
            if let Some(toc) = toc {
                complete_tocs += 1;
                if let Some(first) = first_toc {
                    all_tocs_same &= first == toc;
                } else {
                    first_toc = Some(toc);
                }
            }
        }
        base += got as u64;
    }

    let toc_evidence = if truncated_headers == 0
        && complete_tocs >= 2
        && all_tocs_same
        && first_toc.is_some_and(|toc| toc.iter().any(|byte| *byte != 0))
    {
        TocEvidence::Encrypted
    } else if complete_tocs >= 2 && !all_tocs_same {
        TocEvidence::Different
    } else {
        TocEvidence::Unknown
    };
    Ok(Probe {
        root_magic,
        headers,
        truncated_headers,
        shown,
        toc_evidence,
    })
}

pub(crate) fn run(path: &Path, top: usize) -> i32 {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) => {
            eprintln!("xcl rpf: {}: {error}", path.display());
            return 2;
        }
    };
    let len = match file.metadata() {
        Ok(metadata) => metadata.len(),
        Err(error) => {
            eprintln!("xcl rpf: {}: {error}", path.display());
            return 2;
        }
    };
    let probe = match inspect(&mut file, len, top) {
        Ok(probe) => probe,
        Err(error) => {
            eprintln!("xcl rpf: {}: {error}", path.display());
            return 2;
        }
    };
    if !probe.root_magic {
        eprintln!("xcl rpf: {} is not an RPF9 package", path.display());
        return 2;
    }

    println!(
        "{}: {:.1} MB, streaming header probe",
        path.display(),
        len as f64 / 1048576.0
    );
    print_summary(&probe, top);
    0
}

/// Report headers from archive data already read by the full scan.
pub(crate) fn summarize(mut data: &[u8], top: usize) {
    let len = data.len() as u64;
    let probe = inspect(&mut data, len, top).expect("reading from a slice cannot fail");
    print_summary(&probe, top);
}

fn print_summary(probe: &Probe, top: usize) {
    println!("RPF9 archives (headers every 2 KB): {}", probe.headers);
    for header in &probe.shown {
        if let Some(fields) = header.fields {
            println!(
                "  {:#010x}: fields {:#x} {:#x} {:#x}",
                header.offset, fields[0], fields[1], fields[2]
            );
        } else {
            println!("  {:#010x}: truncated header", header.offset);
        }
    }
    if top > MAX_SHOWN_HEADERS && probe.headers > MAX_SHOWN_HEADERS as u64 {
        println!("header display capped at {MAX_SHOWN_HEADERS}");
    }
    if probe.truncated_headers != 0 {
        println!("truncated RPF9 headers: {}", probe.truncated_headers);
    }
    match probe.toc_evidence {
        TocEvidence::Encrypted => println!(
            "TOC encryption: confirmed indicator (multiple complete archives have the same nonzero TOC prefix)"
        ),
        TocEvidence::Different => println!(
            "TOC encryption: not indicated by header prefixes; contents remain unclassified"
        ),
        TocEvidence::Unknown => println!("TOC encryption: unknown (insufficient complete header evidence)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    struct SyntheticArchive {
        len: u64,
        position: u64,
        bytes: BTreeMap<u64, u8>,
        max_read: usize,
    }

    impl SyntheticArchive {
        fn new(len: u64) -> Self {
            Self {
                len,
                position: 0,
                bytes: BTreeMap::new(),
                max_read: 0,
            }
        }

        fn header(&mut self, offset: u64, toc: [u8; 32]) {
            let mut header = [0u8; 0x60];
            header[..4].copy_from_slice(pack::RPF9_MAGIC);
            header[4..8].copy_from_slice(&0x120u32.to_le_bytes());
            header[8..12].copy_from_slice(&0x100u32.to_le_bytes());
            header[12..16].copy_from_slice(&0x20u32.to_le_bytes());
            header[0x40..0x60].copy_from_slice(&toc);
            for (index, byte) in header.into_iter().enumerate() {
                self.bytes.insert(offset + index as u64, byte);
            }
        }

        fn magic_only(&mut self, offset: u64) {
            for (index, byte) in pack::RPF9_MAGIC.iter().copied().enumerate() {
                self.bytes.insert(offset + index as u64, byte);
            }
        }
    }

    impl Read for SyntheticArchive {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            self.max_read = self.max_read.max(buffer.len());
            let count =
                usize::try_from((self.len - self.position).min(buffer.len() as u64)).unwrap();
            buffer[..count].fill(0);
            let end = self.position + count as u64;
            for (&offset, &byte) in self.bytes.range(self.position..end) {
                buffer[(offset - self.position) as usize] = byte;
            }
            self.position = end;
            Ok(count)
        }
    }

    #[test]
    fn bounds_reads_and_detects_repeated_nonzero_tocs() {
        let logical_len = CHUNK_SIZE as u64 * 3 + 0x1000;
        let mut archive = SyntheticArchive::new(logical_len);
        archive.header(0, [0x5a; 32]);
        archive.header(CHUNK_SIZE as u64 * 2 + 0x800, [0x5a; 32]);

        let probe = inspect(&mut archive, logical_len, 1).unwrap();
        assert!(probe.root_magic);
        assert_eq!(probe.headers, 2);
        assert_eq!(probe.shown.len(), 1);
        assert_eq!(probe.toc_evidence, TocEvidence::Encrypted);
        assert!(archive.max_read <= CHUNK_SIZE);
    }

    #[test]
    fn does_not_guess_from_one_header_or_unbounded_top() {
        let mut archive = SyntheticArchive::new(0x1000);
        archive.header(0, [0x33; 32]);

        let probe = inspect(&mut archive, 0x1000, usize::MAX).unwrap();
        assert!(probe.root_magic);
        assert!(probe.shown.capacity() <= MAX_SHOWN_HEADERS);
        assert_eq!(probe.toc_evidence, TocEvidence::Unknown);
    }

    #[test]
    fn keeps_different_or_zero_tocs_unclassified() {
        let mut different = SyntheticArchive::new(0x1800);
        different.header(0, [0x11; 32]);
        different.header(0x800, [0x22; 32]);
        assert_eq!(
            inspect(&mut different, 0x1800, 2).unwrap().toc_evidence,
            TocEvidence::Different
        );

        let mut zero = SyntheticArchive::new(0x1800);
        zero.header(0, [0; 32]);
        zero.header(0x800, [0; 32]);
        assert_eq!(
            inspect(&mut zero, 0x1800, 2).unwrap().toc_evidence,
            TocEvidence::Unknown
        );
    }

    #[test]
    fn treats_truncated_headers_as_unknown() {
        let mut archive = SyntheticArchive::new(0x804);
        archive.header(0, [0x44; 32]);
        archive.magic_only(0x800);

        let probe = inspect(&mut archive, 0x804, 2).unwrap();
        assert_eq!(probe.headers, 2);
        assert_eq!(probe.truncated_headers, 1);
        assert_eq!(probe.toc_evidence, TocEvidence::Unknown);
    }
}
