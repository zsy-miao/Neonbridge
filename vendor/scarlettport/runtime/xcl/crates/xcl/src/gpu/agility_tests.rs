use super::*;
use crate::pe::{DIR_EXPORT, IMAGE_FILE_DLL, IMAGE_FILE_MACHINE_AMD64};

/// Build a minimal synthetic x64 PE DLL image with an optional `D3D12SDKVersion` export.
fn synthetic_pe_dll(is_dll: bool, export_name: Option<&str>, sdk_version: Option<u32>, forwarder: bool) -> Vec<u8> {
    let mut img = vec![0u8; 0x600];
    // DOS header
    img[0..2].copy_from_slice(b"MZ");
    img[0x3C..0x40].copy_from_slice(&0x80u32.to_le_bytes());
    // PE signature + COFF header at 0x80
    img[0x80..0x84].copy_from_slice(b"PE\0\0");
    let coff = 0x84;
    img[coff..coff + 2].copy_from_slice(&IMAGE_FILE_MACHINE_AMD64.to_le_bytes());
    img[coff + 2..coff + 4].copy_from_slice(&1u16.to_le_bytes()); // 1 section
    let opt_size: u16 = 240;
    img[coff + 16..coff + 18].copy_from_slice(&opt_size.to_le_bytes());
    let chars: u16 = if is_dll { IMAGE_FILE_DLL } else { 0 };
    img[coff + 18..coff + 20].copy_from_slice(&chars.to_le_bytes());

    // Optional header (PE32+) at coff + 20 = 0x98
    let opt = coff + 20;
    img[opt..opt + 2].copy_from_slice(&0x20Bu16.to_le_bytes());
    img[opt + 32..opt + 36].copy_from_slice(&0x1000u32.to_le_bytes()); // SectionAlignment
    img[opt + 36..opt + 40].copy_from_slice(&0x200u32.to_le_bytes()); // FileAlignment
    img[opt + 56..opt + 60].copy_from_slice(&0x2000u32.to_le_bytes()); // SizeOfImage
    img[opt + 60..opt + 64].copy_from_slice(&0x200u32.to_le_bytes()); // SizeOfHeaders
    img[opt + 108..opt + 112].copy_from_slice(&16u32.to_le_bytes()); // NumberOfRvaAndSizes

    // Section table at opt + 240 = 0x188: single .rdata section RVA 0x1000 -> file offset 0x200, size 0x400
    let sec = opt + opt_size as usize;
    img[sec..sec + 6].copy_from_slice(b".rdata");
    img[sec + 8..sec + 12].copy_from_slice(&0x400u32.to_le_bytes()); // VirtualSize
    img[sec + 12..sec + 16].copy_from_slice(&0x1000u32.to_le_bytes()); // VirtualAddress
    img[sec + 16..sec + 20].copy_from_slice(&0x400u32.to_le_bytes()); // SizeOfRawData
    img[sec + 20..sec + 24].copy_from_slice(&0x200u32.to_le_bytes()); // PointerToRawData

    if let Some(name) = export_name {
        // Export directory at RVA 0x1000 (file offset 0x200), size 0x100
        let dir_off = opt + 112 + DIR_EXPORT * 8;
        img[dir_off..dir_off + 4].copy_from_slice(&0x1000u32.to_le_bytes());
        img[dir_off + 4..dir_off + 8].copy_from_slice(&0x100u32.to_le_bytes());

        let base = 0x200;
        let dll_name_rva = 0x1080u32;
        let afunc_rva = 0x1040u32;
        let anames_rva = 0x1050u32;
        let aords_rva = 0x1060u32;
        let exp_name_rva = 0x10A0u32;
        let value_rva = if forwarder { 0x10C0u32 } else { 0x1200u32 };

        img[base + 12..base + 16].copy_from_slice(&dll_name_rva.to_le_bytes());
        img[base + 16..base + 20].copy_from_slice(&1u32.to_le_bytes()); // ordinal base
        img[base + 20..base + 24].copy_from_slice(&1u32.to_le_bytes()); // NumberOfFunctions
        img[base + 24..base + 28].copy_from_slice(&1u32.to_le_bytes()); // NumberOfNames
        img[base + 28..base + 32].copy_from_slice(&afunc_rva.to_le_bytes());
        img[base + 32..base + 36].copy_from_slice(&anames_rva.to_le_bytes());
        img[base + 36..base + 40].copy_from_slice(&aords_rva.to_le_bytes());

        img[0x240..0x244].copy_from_slice(&value_rva.to_le_bytes());
        img[0x250..0x254].copy_from_slice(&exp_name_rva.to_le_bytes());
        img[0x260..0x262].copy_from_slice(&0u16.to_le_bytes());

        let dll_name = b"D3D12Core.dll\0";
        img[0x280..0x280 + dll_name.len()].copy_from_slice(dll_name);
        let nb = name.as_bytes();
        img[0x2A0..0x2A0 + nb.len()].copy_from_slice(nb);
        img[0x2A0 + nb.len()] = 0;

        if forwarder {
            let fwd = b"OTHER.D3D12SDKVersion\0";
            img[0x2C0..0x2C0 + fwd.len()].copy_from_slice(fwd);
        } else if let Some(v) = sdk_version {
            img[0x400..0x404].copy_from_slice(&v.to_le_bytes());
        }
    }
    img
}

#[test]
fn reads_sdk_version_from_exported_symbol() {
    let bytes = synthetic_pe_dll(true, Some("D3D12SDKVersion"), Some(618), false);
    assert_eq!(read_sdk_version_from_bytes(&bytes), Ok(618));
}

#[test]
fn rejects_non_dll_missing_zero_and_forwarder_exports() {
    let exe = synthetic_pe_dll(false, Some("D3D12SDKVersion"), Some(618), false);
    assert!(read_sdk_version_from_bytes(&exe).is_err());

    let no_exports = synthetic_pe_dll(true, None, None, false);
    assert!(read_sdk_version_from_bytes(&no_exports).is_err());

    let other_export = synthetic_pe_dll(true, Some("D3D12GetInterface"), Some(618), false);
    assert!(read_sdk_version_from_bytes(&other_export).is_err());

    let zero_ver = synthetic_pe_dll(true, Some("D3D12SDKVersion"), Some(0), false);
    assert!(read_sdk_version_from_bytes(&zero_ver).is_err());

    let fwd = synthetic_pe_dll(true, Some("D3D12SDKVersion"), Some(618), true);
    assert!(read_sdk_version_from_bytes(&fwd).is_err());
}

#[test]
fn candidate_order_respects_explicit_env_override() {
    let explicit = candidates(
        Some(PathBuf::from(r"D:\custom\agility")),
        Some(PathBuf::from(r"C:\xcl")),
        Some(PathBuf::from(r"C:\Users\u\AppData\Local")),
    );
    assert_eq!(explicit, vec![PathBuf::from(r"D:\custom\agility")]);

    let defaults = candidates(
        None,
        Some(PathBuf::from(r"C:\xcl")),
        Some(PathBuf::from(r"C:\Users\u\AppData\Local")),
    );
    assert_eq!(
        defaults,
        vec![
            PathBuf::from(r"C:\xcl\D3D12"),
            PathBuf::from(r"C:\xcl"),
            PathBuf::from(r"C:\Users\u\AppData\Local\XCL\D3D12"),
        ]
    );
}

#[test]
fn format_sdk_path_ensures_trailing_separator() {
    let p1 = format_sdk_path(Path::new(r"C:\agility")).unwrap();
    assert_eq!(p1.to_str().unwrap(), r"C:\agility\");

    let p2 = format_sdk_path(Path::new(r"C:\agility\")).unwrap();
    assert_eq!(p2.to_str().unwrap(), r"C:\agility\");

    assert!(format_sdk_path(Path::new("")).is_err());
}

#[test]
fn encode_wide_supports_utf8_and_legacy_ansi_code_pages() {
    use windows_sys::Win32::Globalization::CP_UTF8;

    let ascii: Vec<u16> = r"C:\agility\".encode_utf16().collect();
    assert_eq!(encode_wide_for_code_page(&ascii, CP_UTF8).unwrap().to_str().unwrap(), r"C:\agility\");
    assert_eq!(encode_wide_for_code_page(&ascii, 1252).unwrap().to_str().unwrap(), r"C:\agility\");

    // Non-ASCII Unicode succeeds under UTF-8 and is rejected without lossy replacement under 1252.
    let unicode: Vec<u16> = r"C:\ゲーム\D3D12\".encode_utf16().collect();
    assert_eq!(encode_wide_for_code_page(&unicode, CP_UTF8).unwrap().to_str().unwrap(), r"C:\ゲーム\D3D12\");
    assert!(encode_wide_for_code_page(&unicode, 1252).is_err());

    // Unpaired UTF-16 surrogate must be rejected under UTF-8.
    let bad_surrogate: Vec<u16> = vec![b'C' as u16, b':' as u16, 0xD800, b'\\' as u16];
    assert!(encode_wide_for_code_page(&bad_surrogate, CP_UTF8).is_err());
}

#[test]
fn rejects_malformed_export_rvas_and_cross_section_reads() {
    let opt = 0x98usize;
    let dir_off = opt + 112 + DIR_EXPORT * 8;

    // Export directory RVA near u32::MAX must not panic or wrap.
    let mut overflow_dir = synthetic_pe_dll(true, Some("D3D12SDKVersion"), Some(618), false);
    overflow_dir[dir_off..dir_off + 4].copy_from_slice(&(u32::MAX - 4).to_le_bytes());
    overflow_dir[dir_off + 4..dir_off + 8].copy_from_slice(&0x100u32.to_le_bytes());
    assert!(read_sdk_version_from_bytes(&overflow_dir).is_err());

    // Export directory smaller than IMAGE_EXPORT_DIRECTORY (40 bytes) must be rejected.
    let mut short_dir = synthetic_pe_dll(true, Some("D3D12SDKVersion"), Some(618), false);
    short_dir[dir_off + 4..dir_off + 8].copy_from_slice(&20u32.to_le_bytes());
    assert!(read_sdk_version_from_bytes(&short_dir).is_err());

    // D3D12SDKVersion RVA placed in the last 2 bytes of .rdata must not bleed into trailing overlay bytes.
    let mut boundary = synthetic_pe_dll(true, Some("D3D12SDKVersion"), Some(618), false);
    boundary.extend_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD]); // file overlay after .rdata
    // .rdata is RVA 0x1000..0x1400 (file 0x200..0x600); point value_rva at 0x13FE (only 2 bytes left in section)
    boundary[0x240..0x244].copy_from_slice(&0x13FEu32.to_le_bytes());
    boundary[0x5FE..0x600].copy_from_slice(&[0x6A, 0x02]);
    assert!(read_sdk_version_from_bytes(&boundary).is_err());

    // 32-bit x86 DLL (machine 0x014C) must be rejected.
    let mut x86 = synthetic_pe_dll(true, Some("D3D12SDKVersion"), Some(618), false);
    x86[0x84..0x86].copy_from_slice(&0x014Cu16.to_le_bytes());
    assert!(read_sdk_version_from_bytes(&x86).is_err());
}

#[test]
fn env_trimming_trailing_slash_and_no_sm66_messages() {
    use std::ffi::OsString;

    assert_eq!(clean_env_path(None), None);
    assert_eq!(clean_env_path(Some(OsString::from(""))), None);
    assert_eq!(clean_env_path(Some(OsString::from("   \t "))), None);
    assert_eq!(clean_env_path(Some(OsString::from(r" C:\agility "))), Some(PathBuf::from(r"C:\agility")));

    // Trailing forward slash is normalized to a trailing backslash.
    let p = format_sdk_path(Path::new("C:/agility/")).unwrap();
    assert_eq!(p.to_str().unwrap(), r"C:/agility\");

    let msg_not_found = format_no_sm66_message("GPU0", &State::NotFound);
    assert!(msg_not_found.contains(ENV));
    let msg_unavail = format_no_sm66_message("GPU0", &State::Unavailable("bad dll".into()));
    assert!(msg_unavail.contains("bad dll"));
    assert!(!msg_unavail.contains("on Windows 10, set"));
}

#[test]
fn resolve_dir_handles_explicit_override_and_implicit_fallback() {
    let root = std::env::temp_dir().join(format!("xcl_agility_test_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let exe_dir = root.join("exe");
    let exe_d3d12 = exe_dir.join("D3D12");
    let local_app = root.join("local");
    let local_d3d12 = local_app.join("XCL").join("D3D12");
    let empty_dir = root.join("empty");
    std::fs::create_dir_all(&exe_d3d12).unwrap();
    std::fs::create_dir_all(&local_d3d12).unwrap();
    std::fs::create_dir_all(&empty_dir).unwrap();

    // All empty -> NotFound
    assert_eq!(resolve_dir(None, Some(exe_dir.clone()), Some(local_app.clone())), DirResolution::NotFound);

    // Explicit env override pointing at empty dir -> Unavailable (does NOT fall through to local_app)
    let valid_dll = synthetic_pe_dll(true, Some("D3D12SDKVersion"), Some(620), false);
    std::fs::write(local_d3d12.join("D3D12Core.dll"), &valid_dll).unwrap();
    assert!(matches!(
        resolve_dir(Some(empty_dir.clone()), Some(exe_dir.clone()), Some(local_app.clone())),
        DirResolution::Unavailable(_)
    ));

    // Implicit search skips bad DLL in exe\D3D12 and falls through to valid DLL in %LOCALAPPDATA%\XCL\D3D12
    std::fs::write(exe_d3d12.join("D3D12Core.dll"), b"not a pe").unwrap();
    assert_eq!(
        resolve_dir(None, Some(exe_dir.clone()), Some(local_app.clone())),
        DirResolution::Found { dir: local_d3d12.clone(), version: 620 }
    );

    // Implicit search with only a bad DLL returns Unavailable(last_err)
    let _ = std::fs::remove_file(local_d3d12.join("D3D12Core.dll"));
    assert!(matches!(
        resolve_dir(None, Some(exe_dir.clone()), Some(local_app.clone())),
        DirResolution::Unavailable(_)
    ));

    let _ = std::fs::remove_dir_all(&root);
}
