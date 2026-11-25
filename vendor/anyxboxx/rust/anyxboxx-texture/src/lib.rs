//! Textures de la Xbox Series X : blocs BC7, agencement GFX10 en tuiles de 4 Kio, export PNG.
//!
//! Le décodage BC7 et le désentrelacement sont portés de Xbox2Windows (R0Y00, licence MIT,
//! <https://github.com/R0Y00/Xbox2Windows>, `tools/bc7_blocks.py` et `tools/bc7_tiles.py`).
pub mod bc7;
pub mod png;
pub mod tiles;

/// Image RGBA8 (ligne par ligne) d'une surface de blocs BC7 rangés ligne par ligne.
pub fn decode_surface(blocks: &[u8], width: usize, height: usize) -> Result<Vec<u8>, String> {
    const SIDE: usize = 4;
    const PIXEL_BYTES: usize = 4;
    let (columns, rows) = (width.div_ceil(SIDE), height.div_ceil(SIDE));
    if blocks.len() != columns * rows * bc7::BLOCK_BYTES {
        return Err(format!("{} octets reçus, {} attendus pour {width}×{height}", blocks.len(),
                           columns * rows * bc7::BLOCK_BYTES));
    }
    let mut image = vec![0u8; width * height * PIXEL_BYTES];
    if blocks.is_empty() {
        return Ok(image);
    }
    let stride = width * PIXEL_BYTES;
    for (block_y, block_row) in blocks.chunks_exact(columns * bc7::BLOCK_BYTES).enumerate() {
        let y = block_y * SIDE;
        let visible_rows = (height - y).min(SIDE);
        for (block_x, block) in block_row.chunks_exact(bc7::BLOCK_BYTES).enumerate() {
            let texels = bc7::decode(block)?;
            let x = block_x * SIDE;
            let visible_bytes = (width - x).min(SIDE) * PIXEL_BYTES;
            let start = y * stride + x * PIXEL_BYTES;
            for (dy, row) in texels.chunks_exact(SIDE * PIXEL_BYTES).take(visible_rows).enumerate() {
                let at = start + dy * stride;
                image[at..at + visible_bytes].copy_from_slice(&row[..visible_bytes]);
            }
        }
    }
    Ok(image)
}
