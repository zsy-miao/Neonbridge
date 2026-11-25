//! XAccessibility – closed caption properties and high-contrast mode queries (`XAccessibility.h`).
//!
//! Exported directly for titles that import these functions by name, and used as the built-in
//! COM fallback for `QueryApiImpl(XACCESSIBILITY, ...)` when `System32\xgameruntime.dll` is not
//! installed (mirroring the `XAsync` fallback in `api.rs`).

#![allow(non_snake_case)]

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::util::{E_INVALIDARG, HRESULT, S_OK};

/// `XColor` (`XGameRuntimeTypes.h`): `union { struct { uint8_t A, R, G, B; }; uint32_t Value; }`.
#[repr(C, align(4))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct XColor {
    pub a: u8,
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// `XClosedCaptionProperties` (`XAccessibility.h`, 28 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct XClosedCaptionProperties {
    pub background_color: XColor,
    pub font_color: XColor,
    pub window_color: XColor,
    /// `XClosedCaptionFontEdgeAttribute` (`0` = Default)
    pub font_edge_attribute: u32,
    /// `XClosedCaptionFontStyle` (`0` = Default)
    pub font_style: u32,
    pub font_scale: f32,
    /// C++ `bool` (`0` = false, `1` = true)
    pub enabled: u8,
    pub _pad: [u8; 3],
}

/// `XHighContrastMode` (`XAccessibility.h`).
pub const XHIGH_CONTRAST_MODE_OFF: u32 = 0;
pub const XHIGH_CONTRAST_MODE_DARK: u32 = 1;
pub const XHIGH_CONTRAST_MODE_LIGHT: u32 = 2;
#[allow(dead_code)]
pub const XHIGH_CONTRAST_MODE_OTHER: u32 = 3;

static CLOSED_CAPTION_ENABLED: AtomicBool = AtomicBool::new(false);

const HCF_HIGHCONTRASTON: u32 = 0x0000_0001;

#[repr(C)]
struct HighContrastW {
    cb_size: u32,
    dw_flags: u32,
    lpsz_default_scheme: *mut u16,
}

fn default_closed_caption_properties(enabled: bool) -> XClosedCaptionProperties {
    XClosedCaptionProperties {
        background_color: XColor { a: 0xFF, r: 0x00, g: 0x00, b: 0x00 },
        font_color: XColor { a: 0xFF, r: 0xFF, g: 0xFF, b: 0xFF },
        window_color: XColor { a: 0x00, r: 0x00, g: 0x00, b: 0x00 },
        font_edge_attribute: 0,
        font_style: 0,
        font_scale: 1.0,
        enabled: u8::from(enabled),
        _pad: [0; 3],
    }
}

fn classify_high_contrast(enabled: bool, window_colorref: u32) -> u32 {
    if !enabled {
        return XHIGH_CONTRAST_MODE_OFF;
    }
    // Win32 COLORREF is 0x00BBGGRR.
    let r = window_colorref & 0xFF;
    let g = (window_colorref >> 8) & 0xFF;
    let b = (window_colorref >> 16) & 0xFF;
    // Rec.601 luma scaled by 1000 (range 0..=255_000; midpoint 127_500).
    if 299 * r + 587 * g + 114 * b < 127_500 {
        XHIGH_CONTRAST_MODE_DARK
    } else {
        XHIGH_CONTRAST_MODE_LIGHT
    }
}

pub unsafe extern "system" fn XClosedCaptionGetProperties(properties: *mut XClosedCaptionProperties) -> HRESULT {
    if properties.is_null() {
        return E_INVALIDARG;
    }
    let enabled = CLOSED_CAPTION_ENABLED.load(Ordering::Acquire);
    std::ptr::write(properties, default_closed_caption_properties(enabled));
    S_OK
}

pub unsafe extern "system" fn XClosedCaptionSetEnabled(enabled: u8) -> HRESULT {
    CLOSED_CAPTION_ENABLED.store(enabled != 0, Ordering::Release);
    S_OK
}

pub unsafe extern "system" fn XHighContrastGetMode(mode: *mut u32) -> HRESULT {
    if mode.is_null() {
        return E_INVALIDARG;
    }
    use windows::Win32::Graphics::Gdi::{GetSysColor, COLOR_WINDOW};
    use windows_sys::Win32::UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_GETHIGHCONTRAST};

    let mut hc = HighContrastW {
        cb_size: std::mem::size_of::<HighContrastW>() as u32,
        dw_flags: 0,
        lpsz_default_scheme: std::ptr::null_mut(),
    };
    let ok = SystemParametersInfoW(
        SPI_GETHIGHCONTRAST,
        hc.cb_size,
        &mut hc as *mut HighContrastW as *mut c_void,
        0,
    );
    if ok == 0 {
        return crate::util::E_FAIL;
    }
    let enabled = (hc.dw_flags & HCF_HIGHCONTRASTON) != 0;
    let color = if enabled { GetSysColor(COLOR_WINDOW) } else { 0 };
    std::ptr::write(mode, classify_high_contrast(enabled, color));
    S_OK
}

#[cfg(test)]
pub(crate) static ACCESSIBILITY_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_caption_properties_abi_layout() {
        assert_eq!(std::mem::size_of::<XColor>(), 4);
        assert_eq!(std::mem::align_of::<XColor>(), 4);
        assert_eq!(std::mem::size_of::<XClosedCaptionProperties>(), 28);
        assert_eq!(std::mem::align_of::<XClosedCaptionProperties>(), 4);
        assert_eq!(std::mem::offset_of!(XClosedCaptionProperties, background_color), 0x00);
        assert_eq!(std::mem::offset_of!(XClosedCaptionProperties, font_color), 0x04);
        assert_eq!(std::mem::offset_of!(XClosedCaptionProperties, window_color), 0x08);
        assert_eq!(std::mem::offset_of!(XClosedCaptionProperties, font_edge_attribute), 0x0C);
        assert_eq!(std::mem::offset_of!(XClosedCaptionProperties, font_style), 0x10);
        assert_eq!(std::mem::offset_of!(XClosedCaptionProperties, font_scale), 0x14);
        assert_eq!(std::mem::offset_of!(XClosedCaptionProperties, enabled), 0x18);

        // Verify exact ARGB byte layout independently of Rust field names.
        let c = XColor { a: 0x11, r: 0x22, g: 0x33, b: 0x44 };
        let bytes: [u8; 4] = unsafe { std::mem::transmute(c) };
        assert_eq!(bytes, [0x11, 0x22, 0x33, 0x44]);
    }

    #[test]
    fn closed_caption_null_and_roundtrip_zeroes_padding() {
        let _g = ACCESSIBILITY_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            assert_eq!(XClosedCaptionGetProperties(std::ptr::null_mut()), E_INVALIDARG);
            assert_eq!(XClosedCaptionSetEnabled(0), S_OK);

            // Poison the entire destination buffer to verify all 28 bytes (including padding) are written.
            #[repr(C, align(4))]
            struct Aligned([u8; std::mem::size_of::<XClosedCaptionProperties>()]);
            let mut raw = Aligned([0xAAu8; std::mem::size_of::<XClosedCaptionProperties>()]);
            assert_eq!(XClosedCaptionGetProperties(raw.0.as_mut_ptr() as *mut XClosedCaptionProperties), S_OK);
            let props: XClosedCaptionProperties = std::ptr::read(raw.0.as_ptr() as *const XClosedCaptionProperties);
            assert_eq!(props.background_color, XColor { a: 0xFF, r: 0, g: 0, b: 0 });
            assert_eq!(props.font_color, XColor { a: 0xFF, r: 0xFF, g: 0xFF, b: 0xFF });
            assert_eq!(props.window_color, XColor { a: 0, r: 0, g: 0, b: 0 });
            assert_eq!(props.font_edge_attribute, 0);
            assert_eq!(props.font_style, 0);
            assert_eq!(props.font_scale, 1.0);
            assert_eq!(props.enabled, 0);
            assert_eq!(&raw.0[0x00..0x04], &[0xFF, 0x00, 0x00, 0x00]);
            assert_eq!(&raw.0[0x04..0x08], &[0xFF, 0xFF, 0xFF, 0xFF]);
            assert_eq!(&raw.0[0x08..0x0C], &[0x00, 0x00, 0x00, 0x00]);
            assert_eq!(&raw.0[0x14..0x18], &1.0f32.to_le_bytes());
            assert_eq!(raw.0[0x18], 0);
            assert_eq!(&raw.0[0x19..0x1C], &[0, 0, 0]);

            // Any non-zero byte passed from C/C++ normalizes to 1 without UB.
            assert_eq!(XClosedCaptionSetEnabled(42), S_OK);
            assert_eq!(XClosedCaptionGetProperties(raw.0.as_mut_ptr() as *mut XClosedCaptionProperties), S_OK);
            let props: XClosedCaptionProperties = std::ptr::read(raw.0.as_ptr() as *const XClosedCaptionProperties);
            assert_eq!(props.enabled, 1);
            assert_eq!(raw.0[0x18], 1);
            assert_eq!(&raw.0[0x19..0x1C], &[0, 0, 0]);

            assert_eq!(XClosedCaptionSetEnabled(0), S_OK);
        }
    }

    #[test]
    fn high_contrast_classification_and_query() {
        assert_eq!(classify_high_contrast(false, 0x0000_0000), XHIGH_CONTRAST_MODE_OFF);
        assert_eq!(classify_high_contrast(false, 0x00FF_FFFF), XHIGH_CONTRAST_MODE_OFF);
        // Black / Windows 11 Aquatic (#202020) / Dusk (#2D3236) -> Dark
        assert_eq!(classify_high_contrast(true, 0x0000_0000), XHIGH_CONTRAST_MODE_DARK);
        assert_eq!(classify_high_contrast(true, 0x0020_2020), XHIGH_CONTRAST_MODE_DARK);
        assert_eq!(classify_high_contrast(true, 0x0036_322D), XHIGH_CONTRAST_MODE_DARK);
        // Midpoint boundary: RGB(127, 127, 127) -> luma 127_000 < 127_500 (Dark); RGB(128, 128, 128) -> 128_000 (Light)
        assert_eq!(classify_high_contrast(true, 0x007F_7F7F), XHIGH_CONTRAST_MODE_DARK);
        assert_eq!(classify_high_contrast(true, 0x0080_8080), XHIGH_CONTRAST_MODE_LIGHT);
        // White / Windows 11 Desert (#FFFAEF -> COLORREF 0x00EFFAFF) -> Light
        assert_eq!(classify_high_contrast(true, 0x00FF_FFFF), XHIGH_CONTRAST_MODE_LIGHT);
        assert_eq!(classify_high_contrast(true, 0x00EF_FAFF), XHIGH_CONTRAST_MODE_LIGHT);

        unsafe {
            assert_eq!(XHighContrastGetMode(std::ptr::null_mut()), E_INVALIDARG);
            let mut mode = u32::MAX;
            assert_eq!(XHighContrastGetMode(&mut mode), S_OK);
            assert!(mode <= XHIGH_CONTRAST_MODE_OTHER, "unexpected high contrast mode {mode}");
        }
    }
}
