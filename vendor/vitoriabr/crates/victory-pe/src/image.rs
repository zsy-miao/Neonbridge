use object::LittleEndian as LE;
use object::Object;
use object::pe::{self, ImageNtHeaders64};
use object::read::pe::{
    ExportTarget, ImageNtHeaders, ImageOptionalHeader, Import, ImportTable, ImportThunkList,
    PeFile64,
};

use crate::layout::{self, dir};
use crate::{MACHINE_AMD64, PeError};

/// Everything we need to know about a PE32+ image, extracted in one pass.
#[derive(Debug, Clone)]
pub struct ImageInfo {
    pub machine: u16,
    /// `IMAGE_FILE_HEADER::Characteristics`.
    pub characteristics: u16,
    pub timestamp: u32,
    pub subsystem: u16,
    /// `IMAGE_OPTIONAL_HEADER64::DllCharacteristics`.
    pub dll_characteristics: u16,
    pub image_base: u64,
    pub size_of_image: u32,
    pub entry_rva: u32,
    pub stack_reserve: u64,
    pub stack_commit: u64,
    pub sections: Vec<SectionInfo>,
    pub imports: Vec<ImportModule>,
    pub delay_imports: Vec<ImportModule>,
    pub exports: Vec<ExportInfo>,
    pub tls: Option<TlsInfo>,
    pub has_relocations: bool,
    pub has_exception_table: bool,
    pub has_load_config: bool,
    pub codeview: Option<CodeViewInfo>,
}

#[derive(Debug, Clone)]
pub struct SectionInfo {
    pub name: String,
    pub rva: u32,
    pub virtual_size: u32,
    pub file_offset: u32,
    pub file_size: u32,
    pub characteristics: u32,
}

/// One imported DLL and the functions taken from it.
#[derive(Debug, Clone)]
pub struct ImportModule {
    /// DLL name exactly as written in the image (case preserved).
    pub dll: String,
    pub entries: Vec<ImportEntry>,
}

#[derive(Debug, Clone)]
pub struct ImportEntry {
    pub symbol: ImportSymbol,
    /// RVA of the 8-byte IAT slot the loader must fill in.
    pub iat_slot_rva: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ImportSymbol {
    Name { hint: u16, name: String },
    Ordinal(u16),
}

impl std::fmt::Display for ImportSymbol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImportSymbol::Name { name, .. } => f.write_str(name),
            ImportSymbol::Ordinal(o) => write!(f, "#{o}"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ExportInfo {
    pub name: Option<String>,
    pub ordinal: u16,
    /// `None` for forwarded exports.
    pub rva: Option<u32>,
    /// `"dll.name"` or `"dll.#ordinal"` for forwarded exports.
    pub forward: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TlsInfo {
    /// RVA of `IMAGE_TLS_DIRECTORY64`.
    pub directory_rva: u32,
    /// VA (at the preferred base) of the callback array, 0 if none.
    pub callbacks_va: u64,
    /// Callback RVAs, in call order.
    pub callback_rvas: Vec<u32>,
}

/// CodeView (RSDS) record: identifies the exact build that produced an image.
#[derive(Debug, Clone)]
pub struct CodeViewInfo {
    pub pdb_path: String,
    pub guid: [u8; 16],
    pub age: u32,
}

impl CodeViewInfo {
    /// The key symbol servers use: GUID (Windows byte order) + age.
    pub fn symbol_key(&self) -> String {
        let g = &self.guid;
        let d1 = u32::from_le_bytes([g[0], g[1], g[2], g[3]]);
        let d2 = u16::from_le_bytes([g[4], g[5]]);
        let d3 = u16::from_le_bytes([g[6], g[7]]);
        let mut s = format!("{d1:08X}{d2:04X}{d3:04X}");
        for b in &g[8..] {
            s.push_str(&format!("{b:02X}"));
        }
        s.push_str(&format!("{:X}", self.age));
        s
    }
}

/// Upper bound on TLS callbacks we are willing to walk (defends against garbage).
const MAX_TLS_CALLBACKS: usize = 1024;

impl ImageInfo {
    /// Parses a complete PE32+ x86-64 image from its file bytes.
    pub fn parse(data: &[u8]) -> Result<Self, PeError> {
        let kind = object::FileKind::parse(data)?;
        if kind != object::FileKind::Pe64 {
            return Err(PeError::Unsupported(format!(
                "expected PE32+, found {kind:?}"
            )));
        }
        let file = PeFile64::parse(data)?;
        let nt = file.nt_headers();
        let fh = nt.file_header();
        let opt = nt.optional_header();

        let machine = fh.machine.get(LE).0;
        if machine != MACHINE_AMD64 {
            return Err(PeError::Unsupported(format!(
                "machine 0x{machine:04x} is not AMD64"
            )));
        }

        let sections = file
            .section_table()
            .iter()
            .map(|s| SectionInfo {
                name: String::from_utf8_lossy(s.raw_name()).into_owned(),
                rva: s.virtual_address.get(LE),
                virtual_size: s.virtual_size.get(LE),
                file_offset: s.pointer_to_raw_data.get(LE),
                file_size: s.size_of_raw_data.get(LE),
                characteristics: s.characteristics.get(LE).0,
            })
            .collect();

        let dir_present = |index: usize| {
            file.data_directory(index)
                .is_some_and(|d| d.virtual_address.get(LE) != 0 && d.size.get(LE) != 0)
        };

        let imports = match file.import_table()? {
            Some(table) => parse_imports(&table, opt.size_of_image())?,
            None => Vec::new(),
        };
        let delay_imports = parse_delay_imports(&file, opt.size_of_image())?;
        let exports = parse_exports(&file)?;
        let tls = parse_tls(&file, data, opt.image_base())?;
        let codeview = file.pdb_info()?.map(|cv| CodeViewInfo {
            pdb_path: String::from_utf8_lossy(cv.path()).into_owned(),
            guid: cv.guid(),
            age: cv.age(),
        });

        Ok(ImageInfo {
            machine,
            characteristics: fh.characteristics.get(LE).0,
            timestamp: fh.time_date_stamp.get(LE),
            subsystem: opt.subsystem().0,
            dll_characteristics: opt.dll_characteristics().0,
            image_base: opt.image_base(),
            size_of_image: opt.size_of_image(),
            entry_rva: opt.address_of_entry_point(),
            stack_reserve: opt.size_of_stack_reserve(),
            stack_commit: opt.size_of_stack_commit(),
            sections,
            imports,
            delay_imports,
            exports,
            tls,
            has_relocations: dir_present(dir::BASERELOC)
                && !has_flag(
                    fh.characteristics.get(LE).0,
                    layout::file_flags::RELOCS_STRIPPED,
                ),
            has_exception_table: dir_present(dir::EXCEPTION),
            has_load_config: dir_present(dir::LOAD_CONFIG),
            codeview,
        })
    }

    pub fn is_dll(&self) -> bool {
        has_flag(self.characteristics, layout::file_flags::DLL)
    }

    /// Maps an RVA to an offset in the file, if it is backed by file data.
    pub fn rva_to_file_offset(&self, rva: u32) -> Option<u32> {
        self.sections.iter().find_map(|s| {
            let delta = rva.checked_sub(s.rva)?;
            // Raw data may be padded past the virtual size; that padding is
            // not part of the section in memory.
            let backed = match s.virtual_size {
                0 => s.file_size,
                vs => vs.min(s.file_size),
            };
            // Header values are untrusted: an offset near 4 GiB overflows.
            (delta < backed).then(|| s.file_offset.checked_add(delta))?
        })
    }

    pub fn import_count(&self) -> usize {
        self.imports.iter().map(|m| m.entries.len()).sum()
    }

    pub fn subsystem_name(&self) -> &'static str {
        use layout::subsystem::*;
        match self.subsystem {
            WINDOWS_GUI => "windows-gui",
            WINDOWS_CUI => "windows-cui",
            XBOX => "xbox",
            XBOX_CODE_CATALOG => "xbox-code-catalog",
            _ => "other",
        }
    }
}

fn has_flag(value: u16, flag: u16) -> bool {
    value & flag != 0
}

fn utf8(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn convert_import(import: Import<'_>) -> ImportSymbol {
    match import {
        Import::Ordinal(o) => ImportSymbol::Ordinal(o),
        Import::Name(hint, name) => ImportSymbol::Name {
            hint,
            name: utf8(name),
        },
    }
}

fn parse_imports(
    table: &ImportTable<'_>,
    size_of_image: u32,
) -> Result<Vec<ImportModule>, PeError> {
    let mut modules = Vec::new();
    let mut descriptors = table.descriptors()?;
    while let Some(desc) = descriptors.next()? {
        let iat_rva = desc.first_thunk.get(LE);
        // Prefer the unbound name table; fall back to the IAT for old linkers.
        let int_rva = match desc.original_first_thunk.get(LE) {
            0 => iat_rva,
            rva => rva,
        };
        modules.push(ImportModule {
            dll: utf8(table.name(desc.name.get(LE))?),
            entries: entries(table.thunks(int_rva)?, iat_rva, size_of_image, |t| {
                table.import::<ImageNtHeaders64>(t)
            })?,
        });
    }
    Ok(modules)
}

fn parse_delay_imports(
    file: &PeFile64<'_>,
    size_of_image: u32,
) -> Result<Vec<ImportModule>, PeError> {
    let Some(table) = file.delay_load_import_table()? else {
        return Ok(Vec::new());
    };
    let mut modules = Vec::new();
    let mut descriptors = table.descriptors()?;
    while let Some(desc) = descriptors.next()? {
        modules.push(ImportModule {
            dll: utf8(table.name(desc.dll_name_rva.get(LE))?),
            entries: entries(
                table.thunks(desc.import_name_table_rva.get(LE))?,
                desc.import_address_table_rva.get(LE),
                size_of_image,
                |t| table.import::<ImageNtHeaders64>(t),
            )?,
        });
    }
    Ok(modules)
}

/// Pairs each name-table entry with its IAT slot. The IAT RVA is not checked
/// by `object`, so every slot is checked to lie inside the image here.
fn entries<'data>(
    mut thunks: ImportThunkList<'data>,
    iat_rva: u32,
    size_of_image: u32,
    import: impl Fn(pe::ImageThunkData64) -> object::read::Result<Import<'data>>,
) -> Result<Vec<ImportEntry>, PeError> {
    let mut entries = Vec::new();
    while let Some(thunk) = thunks.next::<ImageNtHeaders64>()? {
        let iat_slot_rva = u32::try_from(entries.len() * 8)
            .ok()
            .and_then(|offset| iat_rva.checked_add(offset))
            .filter(|&rva| rva.checked_add(8).is_some_and(|end| end <= size_of_image))
            .ok_or_else(|| {
                PeError::Malformed(format!("IAT at {iat_rva:#x} runs outside the image"))
            })?;
        entries.push(ImportEntry {
            symbol: convert_import(import(thunk)?),
            iat_slot_rva,
        });
    }
    Ok(entries)
}

fn parse_exports(file: &PeFile64<'_>) -> Result<Vec<ExportInfo>, PeError> {
    let Some(table) = file.export_table()? else {
        return Ok(Vec::new());
    };
    Ok(table
        .exports()?
        .into_iter()
        // An address of 0 marks an unused slot in the address table (a gap
        // in the ordinals), not an export at the image base.
        .filter(|e| !matches!(e.target, ExportTarget::Address(0)))
        .map(|e| {
            let (rva, forward) = match e.target {
                ExportTarget::Address(rva) => (Some(rva), None),
                ExportTarget::ForwardByName(d, n) => {
                    (None, Some(format!("{}.{}", utf8(d), utf8(n))))
                }
                ExportTarget::ForwardByOrdinal(d, o) => {
                    (None, Some(format!("{}.#{}", utf8(d), o.0)))
                }
            };
            ExportInfo {
                name: e.name.map(utf8),
                ordinal: e.ordinal.0,
                rva,
                forward,
            }
        })
        .collect())
}

fn parse_tls(
    file: &PeFile64<'_>,
    data: &[u8],
    image_base: u64,
) -> Result<Option<TlsInfo>, PeError> {
    let Some(dd) = file.data_directory(pe::IMAGE_DIRECTORY_ENTRY_TLS) else {
        return Ok(None);
    };
    let directory_rva = dd.virtual_address.get(LE);
    if directory_rva == 0 {
        return Ok(None);
    }
    let sections = file.section_table();
    let tls = sections
        .pe_data_at(data, directory_rva)
        .ok_or_else(|| PeError::Malformed("TLS directory outside of sections".into()))?;
    let callbacks_va = layout::read_u64(tls, layout::TLS64_ADDRESS_OF_CALLBACKS)
        .ok_or_else(|| PeError::Malformed("truncated TLS directory".into()))?;

    let mut callback_rvas = Vec::new();
    if callbacks_va != 0 {
        let array_rva = va_to_rva(callbacks_va, image_base)?;
        let array = sections
            .pe_data_at(data, array_rva)
            .ok_or_else(|| PeError::Malformed("TLS callback array outside of sections".into()))?;
        for slot in array.as_chunks::<8>().0.iter().take(MAX_TLS_CALLBACKS) {
            let va = u64::from_le_bytes(*slot);
            if va == 0 {
                break;
            }
            callback_rvas.push(va_to_rva(va, image_base)?);
        }
    }
    Ok(Some(TlsInfo {
        directory_rva,
        callbacks_va,
        callback_rvas,
    }))
}

fn va_to_rva(va: u64, image_base: u64) -> Result<u32, PeError> {
    va.checked_sub(image_base)
        .and_then(|rva| u32::try_from(rva).ok())
        .ok_or_else(|| PeError::Malformed(format!("VA 0x{va:x} is outside the image")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The test binary itself is a real MSVC-linked PE32+ image (on Windows;
    /// elsewhere it is not a PE image at all, which `rejects_garbage_without_panicking`
    /// still uses).
    fn own_image() -> Vec<u8> {
        std::fs::read(std::env::current_exe().unwrap()).unwrap()
    }

    // The test binary is a PE image only when built for Windows.
    #[cfg(windows)]
    #[test]
    fn parses_own_test_binary() {
        let info = ImageInfo::parse(&own_image()).unwrap();
        assert_eq!(info.machine, MACHINE_AMD64);
        assert!(!info.is_dll());
        assert!(info.entry_rva != 0);
        assert!(info.has_exception_table, "x64 images always carry .pdata");
        assert!(info.sections.iter().any(|s| s.name.starts_with(".text")));
        let kernel32 = info
            .imports
            .iter()
            .find(|m| m.dll.eq_ignore_ascii_case("kernel32.dll"))
            .expect("Rust std imports kernel32");
        assert!(kernel32.entries.iter().all(|e| e.iat_slot_rva != 0));
        // Rust std uses static TLS, so a TLS directory must be present (its
        // callback array may legitimately be empty).
        assert!(info.tls.is_some());
    }

    #[cfg(windows)]
    #[test]
    fn rva_to_file_offset_maps_entry_point() {
        let data = own_image();
        let info = ImageInfo::parse(&data).unwrap();
        let off = info.rva_to_file_offset(info.entry_rva).unwrap() as usize;
        assert!(off < data.len());
    }

    #[cfg(windows)]
    #[test]
    fn rva_to_file_offset_rejects_offsets_past_4_gib() {
        let mut info = ImageInfo::parse(&own_image()).unwrap();
        let rva = info.sections[0].rva + 0x20;
        assert!(info.rva_to_file_offset(rva).is_some());
        // Used to overflow (a panic in debug builds, a wrong offset in
        // release ones).
        info.sections[0].file_offset = u32::MAX - 0x10;
        assert_eq!(info.rva_to_file_offset(rva), None);
    }

    #[test]
    fn rejects_garbage_without_panicking() {
        assert!(ImageInfo::parse(b"").is_err());
        assert!(ImageInfo::parse(b"MZ not really").is_err());
        let mut truncated = own_image();
        truncated.truncate(0x200);
        assert!(ImageInfo::parse(&truncated).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn rejects_iat_outside_image() {
        let mut data = own_image();
        let dd = layout::data_directory_offset(&data, dir::IMPORT).unwrap();
        let import_rva = layout::read_u32(&data, dd).unwrap();
        let desc = ImageInfo::parse(&data)
            .unwrap()
            .rva_to_file_offset(import_rva)
            .unwrap() as usize;
        // First descriptor's FirstThunk, at the 4 GiB limit: used to overflow.
        data[desc + 16..desc + 20].copy_from_slice(&0xFFFF_FFF8u32.to_le_bytes());
        assert!(matches!(
            ImageInfo::parse(&data),
            Err(PeError::Malformed(_))
        ));
    }
    /// A minimal PE32+ image: one section at RVA 0x1000 holding `body`, with
    /// the export directory (RVA 0x1000, 0x80 bytes) pointing at it.
    fn synthetic_image(body: &[u8]) -> Vec<u8> {
        let mut d = vec![0u8; 0x200];
        d[0..2].copy_from_slice(b"MZ");
        d[0x3C..0x40].copy_from_slice(&0x40u32.to_le_bytes());
        d[0x40..0x44].copy_from_slice(b"PE\0\0");
        d[0x44..0x46].copy_from_slice(&MACHINE_AMD64.to_le_bytes());
        d[0x46..0x48].copy_from_slice(&1u16.to_le_bytes()); // sections
        d[0x54..0x56].copy_from_slice(&240u16.to_le_bytes()); // optional header size
        d[0x56..0x58].copy_from_slice(&0x2022u16.to_le_bytes()); // DLL, large address aware
        let opt = 0x58;
        d[opt..opt + 2].copy_from_slice(&0x20Bu16.to_le_bytes());
        d[opt + 24..opt + 32].copy_from_slice(&0x1_8000_0000u64.to_le_bytes());
        d[opt + 32..opt + 36].copy_from_slice(&0x1000u32.to_le_bytes());
        d[opt + 36..opt + 40].copy_from_slice(&0x200u32.to_le_bytes());
        d[opt + 56..opt + 60].copy_from_slice(&0x2000u32.to_le_bytes());
        d[opt + 60..opt + 64].copy_from_slice(&0x200u32.to_le_bytes());
        d[opt + 108..opt + 112].copy_from_slice(&16u32.to_le_bytes());
        d[opt + 112..opt + 116].copy_from_slice(&0x1000u32.to_le_bytes());
        d[opt + 116..opt + 120].copy_from_slice(&0x80u32.to_le_bytes());
        let sec = opt + 240;
        d[sec..sec + 6].copy_from_slice(b".edata");
        d[sec + 8..sec + 12].copy_from_slice(&0x200u32.to_le_bytes()); // virtual size
        d[sec + 12..sec + 16].copy_from_slice(&0x1000u32.to_le_bytes());
        d[sec + 16..sec + 20].copy_from_slice(&0x200u32.to_le_bytes()); // raw size
        d[sec + 20..sec + 24].copy_from_slice(&0x200u32.to_le_bytes()); // raw offset
        d.extend_from_slice(body);
        d.resize(0x400, 0);
        d
    }

    #[test]
    fn unused_export_slots_are_not_exports() {
        // Ordinal base 1; three address slots: "alpha", an unused hole (0),
        // and "gamma". The Windows loader treats address 0 as "not exported".
        let mut body = vec![0u8; 0x100];
        let put32 =
            |b: &mut Vec<u8>, at: usize, v: u32| b[at..at + 4].copy_from_slice(&v.to_le_bytes());
        put32(&mut body, 12, 0x1040); // name of the DLL
        put32(&mut body, 16, 1); // ordinal base
        put32(&mut body, 20, 3); // number of functions
        put32(&mut body, 24, 2); // number of names
        put32(&mut body, 28, 0x1028); // address table
        put32(&mut body, 32, 0x1034); // name pointer table
        put32(&mut body, 36, 0x103C); // name ordinal table
        put32(&mut body, 0x28, 0x1200);
        put32(&mut body, 0x2C, 0);
        put32(&mut body, 0x30, 0x1204);
        put32(&mut body, 0x34, 0x1044);
        put32(&mut body, 0x38, 0x104A);
        body[0x3C..0x3E].copy_from_slice(&0u16.to_le_bytes());
        body[0x3E..0x40].copy_from_slice(&2u16.to_le_bytes());
        body[0x40..0x44].copy_from_slice(b"t.dl");
        body[0x44..0x4A].copy_from_slice(b"alpha\0");
        body[0x4A..0x50].copy_from_slice(b"gamma\0");

        let info = ImageInfo::parse(&synthetic_image(&body)).unwrap();
        let exports: Vec<_> = info
            .exports
            .iter()
            .map(|e| (e.name.as_deref(), e.ordinal, e.rva))
            .collect();
        assert_eq!(
            exports,
            [
                (Some("alpha"), 1, Some(0x1200)),
                (Some("gamma"), 3, Some(0x1204))
            ]
        );
    }
}
