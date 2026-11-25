//! D3D12.X on host D3D12.
//!
//! D3D12.X records commands from code inlined into the title, which writes
//! packets into the command list while its put pointer is below a limit and
//! calls the list's method otherwise. Our lists keep the limits at zero, so
//! every command arrives here as a call, to be run on host D3D12. The
//! title's GPU addresses are CPU addresses (console memory is unified).
//!
//! [`device`] is the device object titles create, [`dxgi`] its adapter side,
//! [`resource`] heaps and resources, [`fence`] fences, [`queue`] command
//! queues, [`list`] command allocators and lists, [`descriptor`]
//! descriptor heaps, [`root`] root signatures, [`query`] query heaps,
//! [`signature`] command signatures, [`pipeline`] pipeline states,
//! [`frame`] frame pacing, [`object`] what all console graphics objects
//! share, [`console`] which console model the title sees. Planned:
//! `present` (swap chain).

#[cfg(all(windows, feature = "diagnostics"))]
mod capture;
#[cfg(windows)]
mod card;
pub mod console;
#[cfg(all(windows, feature = "diagnostics"))]
mod copystat;
#[cfg(all(windows, feature = "diagnostics"))]
mod profile;
pub mod descriptor;
pub mod device;
#[cfg(windows)]
mod draw;
pub mod dxgi;
pub mod fence;
pub mod frame;
#[cfg(windows)]
mod host;
#[cfg(windows)]
mod indirect;
pub mod list;
#[cfg(windows)]
mod memwait;
#[cfg(windows)]
mod memory;
mod timeline;
pub mod memstat;
pub mod object;
pub mod pipeline;
#[cfg(windows)]
mod primitive;
pub mod query;
pub mod queue;
mod readout;
#[cfg(windows)]
mod rects;
pub mod resource;
pub mod root;
#[cfg(windows)]
mod sampler;
#[cfg(windows)]
pub mod shader_cache;
#[cfg(windows)]
mod shadow;
pub mod signature;
#[cfg(windows)]
mod ambient;
#[cfg(windows)]
mod compose;
#[cfg(windows)]
mod twin;
#[cfg(windows)]
mod texpool;
#[cfg(windows)]
mod texture;
#[cfg(windows)]
mod watch;

#[cfg(windows)]
pub use host::on_failure;
pub use memstat::report as memory_report;
#[cfg(windows)]
pub use watch::{
    title_memory_committed, title_memory_remapped, title_memory_view, title_memory_view_gone, title_memory_written,
    writing as kernel_will_write,
};
