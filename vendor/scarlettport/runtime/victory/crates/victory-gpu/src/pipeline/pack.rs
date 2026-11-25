//! The title's shader archive, read by us. A precompiled pipeline
//! ([`super::precompiled`]) names each of its shaders only by the digest of
//! its container's `HASH` part; the container itself, with the signature
//! parts the translator needs, is in the archive (`shaders.rpf` of the
//! package). The archive is indexed once, digest to record, on a thread of
//! its own (a 400 MB read); the index is kept beside the shader cache so
//! later runs only read the records they need.
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use victory_game::sgd5::{self, Place};

/// The archive's records by digest, and the archive to read them from.
pub(crate) struct Pack {
    path: PathBuf,
    places: HashMap<[u8; 16], Place>,
    file: Mutex<Option<File>>,
}

enum State {
    /// No archive given (tests, tools).
    None,
    Indexing,
    Ready(Arc<Pack>),
    Failed,
}

static PACK: RwLock<State> = RwLock::new(State::None);

/// Why a digest has no container.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Missing {
    NoArchive,
    StillIndexing,
    IndexFailed,
    NotInArchive,
    Unreadable,
}

impl Missing {
    pub(crate) fn why(self) -> &'static str {
        match self {
            Missing::NoArchive => "no shader archive given",
            Missing::StillIndexing => "shader archive still being indexed",
            Missing::IndexFailed => "shader archive could not be indexed",
            Missing::NotInArchive => "shader not in the archive",
            Missing::Unreadable => "shader record unreadable",
        }
    }
}

/// Indexes the archive at `path` on a thread of its own, keeping the index
/// in `index` (a file) across runs. Once per process.
pub fn use_shader_pack(path: PathBuf, index: Option<PathBuf>) {
    if !super::precompiled::wants_archive() {
        return;
    }
    {
        let mut state = PACK.write().unwrap_or_else(|e| e.into_inner());
        if !matches!(*state, State::None) {
            return;
        }
        *state = State::Indexing;
    }
    let spawned = std::thread::Builder::new()
        .name("shader archive index".into())
        .spawn(move || {
            let begun = std::time::Instant::now();
            let made = open(&path, index.as_deref());
            let mut state = PACK.write().unwrap_or_else(|e| e.into_inner());
            *state = match made {
                Ok((pack, from_file)) => {
                    tracing::info!(
                        archive = %path.display(),
                        shaders = pack.places.len(),
                        from_index_file = from_file,
                        millis = begun.elapsed().as_millis() as u64,
                        "shader archive indexed for precompiled pipelines"
                    );
                    State::Ready(Arc::new(pack))
                }
                Err(e) => {
                    tracing::warn!(archive = %path.display(), error = %e, "shader archive not indexed: precompiled pipelines are refused (the title makes them itself)");
                    State::Failed
                }
            };
        });
    if spawned.is_err() {
        *PACK.write().unwrap_or_else(|e| e.into_inner()) = State::Failed;
    }
}

/// The archive's index: from `index` if it is for this archive, else made
/// (and written there). Whether it came from the file.
fn open(path: &Path, index: Option<&Path>) -> std::io::Result<(Pack, bool)> {
    let meta = std::fs::metadata(path)?;
    let stamp = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    let kept = index
        .and_then(|i| std::fs::read(i).ok())
        .and_then(|bytes| sgd5::read_index(&bytes, meta.len(), stamp));
    let from_file = kept.is_some();
    let entries = match kept {
        Some(entries) => entries,
        None => {
            let entries = sgd5::index(&mut File::open(path)?)?;
            if let Some(i) = index {
                if let Some(dir) = i.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let _ = std::fs::write(i, sgd5::write_index(&entries, meta.len(), stamp));
            }
            entries
        }
    };
    Ok((
        Pack {
            path: path.to_owned(),
            places: entries.into_iter().collect(),
            file: Mutex::new(None),
        },
        from_file,
    ))
}

impl Pack {
    fn container(&self, digest: &[u8; 16]) -> Result<Vec<u8>, Missing> {
        let place = self.places.get(digest).ok_or(Missing::NotInArchive)?;
        let mut packed = vec![0u8; place.packed_size as usize];
        let mut file = self.file.lock().unwrap_or_else(|e| e.into_inner());
        let read = (|| -> std::io::Result<()> {
            if file.is_none() {
                *file = Some(File::open(&self.path)?);
            }
            let f = file.as_mut().expect("opened");
            f.seek(SeekFrom::Start(place.packed_offset))?;
            f.read_exact(&mut packed)
        })();
        drop(file);
        read.map_err(|_| Missing::Unreadable)?;
        place.unpack(&packed).ok_or(Missing::Unreadable)
    }
}

/// The container whose `HASH` digest is `digest`, read from the archive.
pub(crate) fn container(digest: &[u8; 16]) -> Result<Vec<u8>, Missing> {
    let pack = match &*PACK.read().unwrap_or_else(|e| e.into_inner()) {
        State::None => return Err(Missing::NoArchive),
        State::Indexing => return Err(Missing::StillIndexing),
        State::Failed => return Err(Missing::IndexFailed),
        State::Ready(pack) => pack.clone(),
    };
    pack.container(digest)
}

/// Indexes `path` on this thread (keeping the index in `index`) and uses
/// it: offline tools, which want every container at once.
pub(crate) fn use_blocking(path: &Path, index: Option<&Path>) -> std::io::Result<usize> {
    let (pack, _) = open(path, index)?;
    let shaders = pack.places.len();
    *PACK.write().unwrap_or_else(|e| e.into_inner()) = State::Ready(Arc::new(pack));
    Ok(shaders)
}

/// Indexes `path` on this thread and uses it (tests).
#[cfg(test)]
pub(crate) fn use_now(path: &Path) {
    use_blocking(path, None).expect("index");
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// An archive of one record: `container` deflated, after a name.
    pub(crate) fn archive_of(containers: &[Vec<u8>]) -> Vec<u8> {
        let mut out = vec![0x5a; 0x30];
        out.extend_from_slice(b"SGD5");
        for (i, c) in containers.iter().enumerate() {
            let name = format!("CS_test{i}_Wrapped");
            let packed = miniz_oxide::deflate::compress_to_vec(c, 6);
            let n = (packed.len() + 12) as u32;
            out.push(name.len() as u8 + 1);
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(&[0, 0x3c]);
            for v in [n, 0x000D_DCE7, n, c.len() as u32] {
                out.extend_from_slice(&v.to_le_bytes());
            }
            out.extend_from_slice(&packed);
        }
        out
    }

    #[test]
    fn an_index_kept_in_a_file_is_used_while_the_archive_is_the_same() {
        let dir = tempfile::tempdir().unwrap();
        let c = crate::pipeline::precompiled::tests::container_with_hash([0x31; 16], b"code");
        let archive = dir.path().join("shaders.rpf");
        std::fs::write(&archive, archive_of(std::slice::from_ref(&c))).unwrap();
        let index = dir.path().join("cache").join("shaders.rpf.index");
        let (made, from_file) = open(&archive, Some(&index)).unwrap();
        assert!(!from_file && index.exists());
        assert_eq!(made.container(&[0x31; 16]), Ok(c.clone()));
        assert_eq!(made.container(&[0x32; 16]), Err(Missing::NotInArchive));
        let (kept, from_file) = open(&archive, Some(&index)).unwrap();
        assert!(from_file);
        assert_eq!(kept.container(&[0x31; 16]), Ok(c));
        // Another archive (another size): the kept index is not used.
        std::fs::write(&archive, archive_of(&[vec![1; 9]])).unwrap();
        let (other, from_file) = open(&archive, Some(&index)).unwrap();
        assert!(!from_file && other.places.is_empty());
    }
}
