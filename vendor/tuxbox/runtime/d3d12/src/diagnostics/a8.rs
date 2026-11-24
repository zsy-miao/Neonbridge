//! SRVs made of A8 textures (re-derived from rebox 84c4d29; TUXBOX_A8_LOG=0
//! off, 16 lines at most): the view as the title wrote it, its component
//! mapping and the resource, since A8 reads only alpha and a wrong swizzle or
//! a translated format reads as black or white.
use std::sync::atomic::{AtomicU32, Ordering};
use windows::Win32::Graphics::Direct3D12::{D3D12_SHADER_RESOURCE_VIEW_DESC, ID3D12Resource};

pub(crate) fn srv(view: Option<&D3D12_SHADER_RESOURCE_VIEW_DESC>, resource: Option<&ID3D12Resource>, final_format: u32) {
    let resource_desc = resource.map(|r| unsafe { r.GetDesc() });
    let a8 = view.is_some_and(|v| v.Format.0 == 65) || (view.is_none_or(|v| v.Format.0 == 0) && resource_desc.is_some_and(|d| d.Format.0 == 65));
    if !a8 || std::env::var_os("TUXBOX_A8_LOG").is_some_and(|v| v == "0") {
        return;
    }
    static LINES: AtomicU32 = AtomicU32::new(0);
    if LINES.fetch_add(1, Ordering::Relaxed) >= 16 {
        return;
    }
    let view_text = view.map_or("default view".to_string(), |v| format!("format {} dimension {} mapping {:#x}",
        v.Format.0, v.ViewDimension.0, v.Shader4ComponentMapping));
    let resource_text = resource_desc.map_or("no resource".to_string(), |d| format!("resource {}x{}x{} mips {} format {} flags {:#x}",
        d.Width, d.Height, d.DepthOrArraySize, d.MipLevels, d.Format.0, d.Flags.0));
    super::log_line(format_args!("A8 SRV: {view_text} -> native format {final_format}; {resource_text}; callers {}", super::audio::callers()));
}
