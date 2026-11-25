use super::reserved_desc;
use windows::Win32::Foundation::{E_INVALIDARG, E_NOTIMPL};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;

fn texture(layout: D3D12_TEXTURE_LAYOUT) -> D3D12_RESOURCE_DESC {
    D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
        Width: 256,
        Height: 256,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: DXGI_FORMAT_R8G8B8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: layout,
        ..Default::default()
    }
}

#[test]
fn accepts_a_64_kib_texture_and_a_row_major_buffer() {
    assert!(reserved_desc(&texture(D3D12_TEXTURE_LAYOUT_64KB_UNDEFINED_SWIZZLE)).is_ok());
    assert!(reserved_desc(&texture(D3D12_TEXTURE_LAYOUT_64KB_STANDARD_SWIZZLE)).is_ok());
    let buffer = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: 65536,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: DXGI_FORMAT_UNKNOWN,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        Alignment: u64::from(D3D12_DEFAULT_RESOURCE_PLACEMENT_ALIGNMENT),
        ..Default::default()
    };
    assert!(reserved_desc(&buffer).is_ok());
}

#[test]
fn refuses_console_layouts_and_ordinary_textures() {
    assert_eq!(reserved_desc(&texture(D3D12_TEXTURE_LAYOUT(0x101))).unwrap_err(), E_NOTIMPL);
    assert_eq!(
        reserved_desc(&texture(D3D12_TEXTURE_LAYOUT_UNKNOWN)).unwrap_err(),
        E_INVALIDARG
    );
    let mut wide = texture(D3D12_TEXTURE_LAYOUT_64KB_UNDEFINED_SWIZZLE);
    wide.Alignment = 4096;
    assert_eq!(reserved_desc(&wide).unwrap_err(), E_INVALIDARG);
    wide.Alignment = 0;
    wide.Width = 0;
    assert_eq!(reserved_desc(&wide).unwrap_err(), E_INVALIDARG);
}
