//! Header artwork. The picture comes from the user's own game folder at run time (the
//! package's splash screen, else its largest logo); the launcher ships no artwork of any title.
//! Without a folder or a picture, a plain dark gradient is used.
use eframe::egui::ColorImage;
use std::path::{Path, PathBuf};

/// The picture to use from a game folder: `xbs_splash.png`, else the largest `xbs_logo_*.png`.
pub fn pick(game: &Path) -> Option<PathBuf> {
    let splash = game.join("xbs_splash.png");
    if splash.is_file() {
        return Some(splash);
    }
    std::fs::read_dir(game)
        .ok()?
        .flatten()
        .filter(|e| {
            let n = e.file_name().to_string_lossy().to_ascii_lowercase();
            n.starts_with("xbs_logo_") && n.ends_with(".png")
        })
        .filter_map(|e| Some((e.metadata().ok()?.len(), e.path())))
        .max_by_key(|(len, _)| *len)
        .map(|(_, p)| p)
}

/// Loads the folder's picture, scaled down to at most 1600 px wide. None when there is none.
pub fn from_game(game: &Path) -> Option<ColorImage> {
    let img = image::open(pick(game)?).ok()?;
    let img = if img.width() > 1600 { img.resize(1600, 1600 * img.height() / img.width().max(1), image::imageops::FilterType::Triangle) } else { img };
    let rgba = img.to_rgba8();
    Some(ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw()))
}

/// A plain dark vertical gradient.
pub fn fallback() -> ColorImage {
    let (w, h) = (64usize, 256usize);
    let mut pixels = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        let t = y as f32 / (h - 1) as f32;
        let c = |a: f32, b: f32| (a + (b - a) * t) as u8;
        for _ in 0..w {
            pixels.extend_from_slice(&[c(34., 12.), c(28., 10.), c(48., 18.), 255]);
        }
    }
    ColorImage::from_rgba_unmultiplied([w, h], &pixels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_splash_then_largest_logo() {
        let d = std::env::temp_dir().join(format!("sp-art-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        assert_eq!(pick(&d), None);
        std::fs::write(d.join("xbs_logo_44.png"), [0u8; 10]).unwrap();
        std::fs::write(d.join("xbs_logo_150.png"), [0u8; 50]).unwrap();
        assert_eq!(pick(&d), Some(d.join("xbs_logo_150.png")));
        std::fs::write(d.join("xbs_splash.png"), [0u8; 5]).unwrap();
        assert_eq!(pick(&d), Some(d.join("xbs_splash.png")));
        assert!(from_game(&d).is_none()); // not a real PNG
        let _ = std::fs::remove_dir_all(&d);
    }
}
