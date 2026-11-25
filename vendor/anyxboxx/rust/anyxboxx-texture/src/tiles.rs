//! Agencement GFX10 « SW_4K_S » (standard, sans XOR) des blocs BC7 de 16 octets.
//!
//! Portage de `tools/bc7_tiles.py` de Xbox2Windows (R0Y00, licence MIT), d'après les tables de
//! motifs publiques d'AMD (addrlib, `gfx10SwizzlePattern.h`). Une tuile de 4 Kio contient
//! 16 × 16 blocs ; bits de l'index du bloc, du faible au fort : y0 y1 x0 x1 y2 x2 y3 x3.
use crate::bc7::BLOCK_BYTES;

pub const TILE_BYTES: usize = 4096;
const TILE_BLOCKS: usize = 16;
const MIP_TAIL_LEVELS: usize = 8;
/// Position (colonne, ligne) de chaque niveau de la queue de mips dans la tuile de 16 × 16 blocs.
const TAIL_ORIGINS: [(usize, usize); MIP_TAIL_LEVELS] =
    [(8, 0), (4, 8), (0, 12), (0, 8), (4, 4), (4, 0), (0, 4), (0, 0)];

pub fn x_bits(x: usize) -> usize {
    ((x & 3) << 2) | ((x & 4) << 3) | ((x & 8) << 4)
}

pub fn y_bits(y: usize) -> usize {
    (y & 3) | ((y & 4) << 2) | ((y & 8) << 3)
}

/// Blocs d'une surface rangée en tuiles SW_4K_S complètes, remis ligne par ligne.
pub fn linearize_4k_s(payload: &[u8], width: usize, height: usize) -> Result<Vec<u8>, String> {
    if width == 0 || height == 0 {
        return Err("dimensions nulles".into());
    }
    let (columns, rows) = (width.div_ceil(4), height.div_ceil(4));
    let tile_pitch = columns.div_ceil(TILE_BLOCKS);
    if payload.len() != tile_pitch * rows.div_ceil(TILE_BLOCKS) * TILE_BYTES {
        return Err("tuiles de 4 Kio incomplètes pour cette surface".into());
    }
    let mut output = vec![0u8; columns * rows * BLOCK_BYTES];
    for y in 0..rows {
        let tile_row = (y / TILE_BLOCKS) * tile_pitch;
        for x in 0..columns {
            let inside = y_bits(y % TILE_BLOCKS) | x_bits(x % TILE_BLOCKS);
            let source = (tile_row + x / TILE_BLOCKS) * TILE_BYTES + inside * BLOCK_BYTES;
            let target = (y * columns + x) * BLOCK_BYTES;
            output[target..target + BLOCK_BYTES].copy_from_slice(&payload[source..source + BLOCK_BYTES]);
        }
    }
    Ok(output)
}

/// Un niveau de la queue de mips partagée dans une tuile de 4 Kio (niveau 0 = le plus grand).
pub fn linearize_4k_s_tail(tile: &[u8], width: usize, height: usize, level: usize) -> Result<Vec<u8>, String> {
    if level >= MIP_TAIL_LEVELS || width == 0 || height == 0 {
        return Err("niveau ou dimensions invalides pour la queue de mips".into());
    }
    let (columns, rows) = (width.div_ceil(4), height.div_ceil(4));
    if columns > (8 >> level).max(1) || rows > (16 >> level).max(1) {
        return Err("ce niveau dépasse sa place dans la tuile".into());
    }
    if tile.len() != TILE_BYTES {
        return Err("une tuile de queue de mips fait 4 Kio".into());
    }
    let (x0, y0) = TAIL_ORIGINS[level];
    let mut output = Vec::with_capacity(columns * rows * BLOCK_BYTES);
    for row in y0..y0 + rows {
        for column in x0..x0 + columns {
            let start = (y_bits(row) | x_bits(column)) * BLOCK_BYTES;
            output.extend_from_slice(&tile[start..start + BLOCK_BYTES]);
        }
    }
    Ok(output)
}
