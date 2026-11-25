//! Lookups over the generated PC D3D12 interface table (`hle/d3d12_layout.rs`).

use windows_sys::core::GUID;

pub use crate::hle::d3d12_layout::Iface;
use crate::hle::d3d12_layout::IFACES;

pub fn find(name: &str) -> Option<&'static Iface> {
    IFACES.iter().find(|i| i.name == name)
}

pub fn slot(iface: &Iface, method: &str) -> Option<usize> {
    iface.methods.iter().position(|m| *m == method)
}

/// `base` and its numbered versions (ID3D12Device, ID3D12Device1, …), most derived first.
pub fn chain(base: &str) -> Vec<&'static Iface> {
    let mut v: Vec<&'static Iface> = IFACES
        .iter()
        .filter(|i| i.name == base || (i.name.starts_with(base) && i.name[base.len()..].chars().all(|c| c.is_ascii_digit())))
        .collect();
    v.sort_by(|a, b| b.methods.len().cmp(&a.methods.len()));
    v
}

pub fn parse_guid(s: &str) -> GUID {
    let hex: String = s.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    GUID::from_u128(u128::from_str_radix(&hex, 16).unwrap_or(0))
}

pub fn guid_string(g: &GUID) -> String {
    format!(
        "{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        g.data1, g.data2, g.data3, g.data4[0], g.data4[1], g.data4[2], g.data4[3], g.data4[4], g.data4[5], g.data4[6], g.data4[7]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_slots_and_chains() {
        let dev = find("ID3D12Device").unwrap();
        assert_eq!(slot(dev, "CreateCommandQueue"), Some(8));
        assert_eq!(slot(dev, "CreateCommittedResource"), Some(27));
        assert_eq!(slot(dev, "CreateFence"), Some(36));
        let q = find("ID3D12CommandQueue").unwrap();
        assert_eq!(slot(q, "ExecuteCommandLists"), Some(10));
        let cl = find("ID3D12GraphicsCommandList").unwrap();
        assert_eq!(slot(cl, "OMSetRenderTargets"), Some(46));
        let c = chain("ID3D12Device");
        assert!(c.len() >= 10 && c[0].methods.len() > c[c.len() - 1].methods.len());
        assert!(c.iter().all(|i| i.name.starts_with("ID3D12Device")));
        assert!(!chain("ID3D12Device").iter().any(|i| i.name == "ID3D12DeviceChild"));
        let g = parse_guid(dev.iid);
        assert_eq!(guid_string(&g), "189819f1-1db6-4b57-be54-1821339b85f7");
    }
}
