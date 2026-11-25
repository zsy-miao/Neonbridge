//! COFF import libraries (`.lib`).
//!
//! An umbrella import library tells us which DLL provides each API that a
//! console title may link against. That is the map the runtime needs to
//! decide which of our implementations answers which import.

use object::read::archive::ArchiveFile;
use object::read::coff::{ImportFile, ImportName, ImportType};

use crate::{ImportSymbol, PeError};

/// One `dll!export` pair described by an import library.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct LibImport {
    pub dll: String,
    pub export: ImportSymbol,
    pub kind: LibImportKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LibImportKind {
    Code,
    Data,
    Const,
}

/// Result of reading an import library.
#[derive(Debug, Default)]
pub struct ImportLibrary {
    pub imports: Vec<LibImport>,
    /// Members that are not short-format import objects (regular objects,
    /// long-format import descriptors). Reported so nothing is silently lost.
    pub other_members: usize,
}

impl ImportLibrary {
    pub fn parse(data: &[u8]) -> Result<Self, PeError> {
        let archive = ArchiveFile::parse(data)?;
        let mut lib = ImportLibrary::default();
        for member in archive.members() {
            let member = member?;
            let bytes = member.data(data)?;
            match ImportFile::parse(bytes) {
                Ok(import) => lib.imports.push(LibImport {
                    dll: String::from_utf8_lossy(import.dll()).into_owned(),
                    export: match import.import() {
                        ImportName::Name(name) => ImportSymbol::Name {
                            hint: 0,
                            name: String::from_utf8_lossy(name).into_owned(),
                        },
                        ImportName::Ordinal(o) => ImportSymbol::Ordinal(o),
                    },
                    kind: match import.import_type() {
                        ImportType::Code => LibImportKind::Code,
                        ImportType::Data => LibImportKind::Data,
                        ImportType::Const => LibImportKind::Const,
                    },
                }),
                Err(_) => lib.other_members += 1,
            }
        }
        lib.imports.sort();
        lib.imports.dedup();
        Ok(lib)
    }
}
