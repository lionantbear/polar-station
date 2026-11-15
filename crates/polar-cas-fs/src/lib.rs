//! Native filesystem adapters for Polar CAS.
//!
//! Positional I/O is always available on Linux. `erofs` enables direct native
//! pack construction and member reads; Catalog/mount integration is separate.
#![deny(unsafe_op_in_unsafe_fn)]
#![cfg(target_os = "linux")]

mod io;
pub use io::{digest_path, Error, Output, PreparedSource, Source};

#[cfg(feature = "erofs")]
mod erofs;
#[cfg(feature = "erofs")]
pub use erofs::{
    native_io_counters, Compression, Image, ImageDescriptor, Member, NativeIoCounters, PackBuilder,
    PackObject, PackOptions, PackProfile, PackResult,
};

#[cfg(feature = "erofs")]
mod native_meta;
#[cfg(feature = "erofs")]
pub use native_meta::{
    MetaEntry, MetaKind, MetaOptions, MetaStat, NativeMeta, NativeMetaCompiler, NativeMetaResult,
};

#[cfg(feature = "polar")]
pub mod projection;

#[cfg(feature = "polar")]
pub mod host;
pub mod verity;

#[cfg(feature = "polar")]
mod compile_image;
#[cfg(feature = "polar")]
pub use compile_image::{CasImageLease, MAX_COMPILE_SNAPSHOT_BYTES};

#[cfg(feature = "async-reader")]
mod async_reader;
#[cfg(feature = "async-reader")]
pub use async_reader::ReadBudget;
#[cfg(feature = "async-reader")]
mod source_reader;
#[cfg(feature = "async-reader")]
pub use source_reader::ErofsSourceReader;

#[cfg(feature = "async-reader")]
pub mod receive;

#[cfg(feature = "erofs")]
pub use native_meta::{HybridMeta, HybridMetaCompiler};
#[cfg(feature = "polar")]
pub mod hybrid_projection;

#[cfg(feature = "polar")]
pub mod hybrid_host;

#[cfg(feature = "erofs")]
mod coverage;
#[cfg(feature = "erofs")]
pub use coverage::{verify_coverage, CoverageBudget, VerifiedCoverage};
#[cfg(feature = "async-reader")]
pub mod hybrid_receive;
#[cfg(feature = "polar")]
pub mod hybrid_transfer;

#[cfg(feature = "erofs")]
mod direct_input;
#[cfg(feature = "erofs")]
pub use direct_input::{
    DirectPackOptions, IdentifiedSources, IdentityLimits, PackConstruction, SourceIdentity,
    SourceReadStats,
};

mod generated;
pub use generated::seal_generated_file;

#[cfg(feature = "erofs")]
mod filesystem_pack;
#[cfg(feature = "erofs")]
pub use filesystem_pack::{
    FilesystemEntry, FilesystemKind, FilesystemPackBuilder, FilesystemPackOptions,
    FilesystemPackReader, FilesystemPackResult,
};

#[cfg(feature = "async-reader")]
mod filesystem_source;
#[cfg(feature = "async-reader")]
pub use filesystem_source::FilesystemSourceReader;
