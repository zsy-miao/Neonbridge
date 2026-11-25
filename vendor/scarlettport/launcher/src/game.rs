//! Game folder checks, title data folder lookup and cache drive helpers.
//! Nothing here reads file contents beyond MicrosoftGame.config; sizes come from metadata.
use std::path::{Path, PathBuf};

pub const REQUIRED: [&str; 3] = ["game_scarlett_master.exe", "shaders.rpf", "MicrosoftGame.config"];
pub const LOW_SPACE: u64 = 40 * 1024 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileCheck {
    pub name: &'static str,
    pub size: Option<u64>,
}

pub fn verify(dir: &Path) -> Vec<FileCheck> {
    REQUIRED
        .iter()
        .map(|n| FileCheck { name: n, size: std::fs::metadata(dir.join(n)).ok().filter(|m| m.is_file()).map(|m| m.len()) })
        .collect()
}

pub fn missing(checks: &[FileCheck]) -> Vec<&'static str> {
    checks.iter().filter(|c| c.size.is_none()).map(|c| c.name).collect()
}

pub fn human(bytes: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024. && i < U.len() - 1 {
        v /= 1024.;
        i += 1;
    }
    if i == 0 {
        format!("{bytes} B")
    } else if v >= 100. {
        format!("{v:.0} {}", U[i])
    } else {
        format!("{v:.1} {}", U[i])
    }
}

/// "exe 12.3 GB  ·  shaders.rpf 1.2 GB  ·  MicrosoftGame.config 4.1 KB", missing ones flagged.
pub fn summary(checks: &[FileCheck]) -> String {
    checks
        .iter()
        .map(|c| {
            let short = if c.name.ends_with(".exe") { "exe" } else { c.name };
            match c.size {
                Some(s) => format!("{short} {}", human(s)),
                None => format!("{} missing", c.name),
            }
        })
        .collect::<Vec<_>>()
        .join("  ·  ")
}

fn decode_text(raw: &[u8]) -> Option<String> {
    let (body, le) = if let Some(b) = raw.strip_prefix(&[0xFF, 0xFE]) {
        (b, true)
    } else if let Some(b) = raw.strip_prefix(&[0xFE, 0xFF]) {
        (b, false)
    } else {
        return Some(String::from_utf8_lossy(raw.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(raw)).into_owned());
    };
    let units: Vec<u16> = body.chunks_exact(2).map(|b| if le { u16::from_le_bytes([b[0], b[1]]) } else { u16::from_be_bytes([b[0], b[1]]) }).collect();
    String::from_utf16(&units).ok()
}

fn without_comments(xml: &str) -> String {
    let mut out = String::new();
    let mut rest = xml;
    while let Some(i) = rest.find("<!--") {
        out.push_str(&rest[..i]);
        match rest[i + 4..].find("-->") {
            Some(j) => rest = &rest[i + 4 + j + 3..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// `Name` of the `<Identity>` element: the runtime's package name.
pub fn identity(xml: &str) -> Option<String> {
    let xml = without_comments(xml);
    let mut from = 0;
    while let Some(i) = xml[from..].find("<Identity").map(|i| from + i) {
        from = i + 9;
        let rest = &xml[from..];
        if !rest.starts_with(|c: char| c.is_whitespace() || c == '/' || c == '>') {
            continue;
        }
        let end = rest.find('>')?;
        let tag = &rest[..end];
        let mut pos = 0;
        while let Some(k) = tag[pos..].find("Name=\"").map(|k| pos + k) {
            if k == 0 || tag[..k].ends_with(char::is_whitespace) {
                let a = k + 6;
                let b = tag[a..].find('"')?;
                return Some(tag[a..a + b].replace("&amp;", "&"));
            }
            pos = k + 6;
        }
        return None;
    }
    None
}

/// The runtime names its data folder after the package identity, with anything but
/// letters, digits and `._-` replaced by `_` (see `sanitize` in xcl's launcher.rs).
pub fn sanitize(s: &str) -> String {
    let t: String = s.chars().map(|c| if c.is_alphanumeric() || "._-".contains(c) { c } else { '_' }).collect();
    if t.is_empty() {
        "title".into()
    } else {
        t
    }
}

pub fn package_name(game: &Path) -> String {
    let id = std::fs::read(game.join("MicrosoftGame.config")).ok().and_then(|r| decode_text(&r)).and_then(|x| identity(&x));
    sanitize(&id.unwrap_or_else(|| game.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()))
}

/// `%XCL_DATA_ROOT%` or `%LOCALAPPDATA%\XCL`, then the package name.
pub fn title_data_dir(game: &Path) -> PathBuf {
    let root = std::env::var_os("XCL_DATA_ROOT").map(PathBuf::from).unwrap_or_else(|| {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("XCL")
    });
    root.join(package_name(game))
}

pub fn journal_path(data_dir: &Path) -> PathBuf {
    data_dir.join("cache").join("pipelines.bin")
}

/// Size of the pipeline journal when it holds more than its 16 byte header.
pub fn journal_len(game: &Path) -> Option<u64> {
    std::fs::metadata(journal_path(&title_data_dir(game))).ok().map(|m| m.len()).filter(|&l| l > 16)
}

pub fn dxil_dir(cache: &str) -> PathBuf {
    Path::new(cache.trim()).join("dxil")
}

#[cfg(windows)]
mod sys {
    #[link(name = "kernel32")]
    extern "system" {
        pub fn GetLogicalDrives() -> u32;
        pub fn GetDriveTypeW(root: *const u16) -> u32;
        pub fn GetDiskFreeSpaceExW(dir: *const u16, avail: *mut u64, total: *mut u64, free: *mut u64) -> i32;
    }
}

#[cfg(windows)]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Free bytes (available to this user) on the drive holding `path`, using its nearest existing folder.
#[cfg(windows)]
pub fn free_space(path: &Path) -> Option<u64> {
    let mut p = path.to_path_buf();
    while !p.exists() {
        if !p.pop() {
            return None;
        }
    }
    let mut avail = 0u64;
    let (mut total, mut free) = (0u64, 0u64);
    let ok = unsafe { sys::GetDiskFreeSpaceExW(wide(&p.to_string_lossy()).as_ptr(), &mut avail, &mut total, &mut free) };
    (ok != 0).then_some(avail)
}
#[cfg(not(windows))]
pub fn free_space(_: &Path) -> Option<u64> {
    None
}

/// `<fixed drive with the most free space>\ScarlettCache`.
#[cfg(windows)]
pub fn default_cache() -> String {
    let mask = unsafe { sys::GetLogicalDrives() };
    let best = (0..26u8)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| format!("{}:\\", (b'A' + i) as char))
        .filter(|root| unsafe { sys::GetDriveTypeW(wide(root).as_ptr()) } == 3)
        .filter_map(|root| free_space(Path::new(&root)).map(|f| (f, root)))
        .max_by_key(|(f, _)| *f);
    match best {
        Some((_, root)) => format!("{root}ScarlettCache"),
        None => crate::runtime::data_dir().join("cache").display().to_string(),
    }
}
#[cfg(not(windows))]
pub fn default_cache() -> String {
    crate::runtime::data_dir().join("cache").display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_and_sanitize() {
        let xml = r#"<Game><!-- <Identity Name="Old"/> --><Identity Publisher="X" Name="Publisher.Game &amp; Co" Version="1"/></Game>"#;
        assert_eq!(identity(xml).as_deref(), Some("Publisher.Game & Co"));
        assert_eq!(sanitize("Publisher.Game & Co"), "Publisher.Game___Co");
        assert_eq!(sanitize(""), "title");
        assert_eq!(identity("<Game/>"), None);
    }

    #[test]
    fn utf16_config() {
        let mut raw = vec![0xFF, 0xFE];
        for u in r#"<Identity Name="A.B"/>"#.encode_utf16() {
            raw.extend(u.to_le_bytes());
        }
        assert_eq!(identity(&decode_text(&raw).unwrap()).as_deref(), Some("A.B"));
    }

    #[test]
    fn sizes_and_missing() {
        assert_eq!(human(1536), "1.5 KB");
        assert_eq!(human(200 * 1024 * 1024 * 1024), "200 GB");
        let c = vec![FileCheck { name: "a", size: Some(1) }, FileCheck { name: "b", size: None }];
        assert_eq!(missing(&c), vec!["b"]);
    }
}
