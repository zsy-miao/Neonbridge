//! Read-only analysis of PE32+ (x86-64) images and COFF import libraries.
//!
//! Xbox Series X|S titles are ordinary x86-64 PE32+ images (the console CPU
//! is a Zen 2), so the same parser serves titles, console system DLLs and
//! host binaries alike. This crate never modifies anything; patching lives in
//! `victory-loader`, which uses the raw offsets exported from [`layout`].

mod image;
pub mod implib;
pub mod layout;

pub use image::{
    CodeViewInfo, ExportInfo, ImageInfo, ImportEntry, ImportModule, ImportSymbol, SectionInfo,
    TlsInfo,
};

/// Errors produced while parsing an image or import library.
#[derive(Debug, thiserror::Error)]
pub enum PeError {
    /// The file is a valid PE but not one we handle (e.g. 32-bit or ARM64).
    #[error("unsupported image: {0}")]
    Unsupported(String),
    /// The file is truncated or internally inconsistent.
    #[error("malformed image: {0}")]
    Malformed(String),
}

impl From<object::read::Error> for PeError {
    fn from(e: object::read::Error) -> Self {
        PeError::Malformed(e.to_string())
    }
}

/// `IMAGE_FILE_MACHINE_AMD64`.
pub const MACHINE_AMD64: u16 = 0x8664;
