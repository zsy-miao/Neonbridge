//! Blocs BC7 (format Microsoft) : disposition des champs, assemblage et décodage en RGBA8.
//!
//! Portage en Rust de `tools/bc7_blocks.py` de Xbox2Windows (R0Y00, licence MIT,
//! <https://github.com/R0Y00/Xbox2Windows>), d'après la spécification publique de Microsoft
//! (<https://learn.microsoft.com/en-us/windows/win32/direct3d11/bc7-format>). Le décodage
//! lit les bits directement ; les champs nommés (`layout`, `unpack`, `pack`) servent aux
//! vérifications et aux outils.
use std::collections::BTreeMap;

pub const BLOCK_BYTES: usize = 16;
pub const TEXELS: usize = 16;
pub const RGBA_BYTES: usize = TEXELS * 4;
const BLOCK_BITS: u32 = 128;

/// Paramètres d'un mode : sous-ensembles, bits de partition, de rotation, de sélection
/// d'index, de couleur, d'alpha, bits P par extrémité et par sous-ensemble, bits d'index.
#[derive(Clone, Copy, Debug)]
pub struct Mode {
    pub subsets: u32,
    pub partition_bits: u32,
    pub rotation_bits: u32,
    pub selection_bits: u32,
    pub colour_bits: u32,
    pub alpha_bits: u32,
    pub endpoint_p: u32,
    pub subset_p: u32,
    pub index_bits: u32,
    pub index2_bits: u32,
}

#[allow(clippy::too_many_arguments)]
const fn mode(subsets: u32, partition_bits: u32, rotation_bits: u32, selection_bits: u32, colour_bits: u32,
              alpha_bits: u32, endpoint_p: u32, subset_p: u32, index_bits: u32, index2_bits: u32) -> Mode {
    Mode { subsets, partition_bits, rotation_bits, selection_bits, colour_bits, alpha_bits, endpoint_p, subset_p,
           index_bits, index2_bits }
}

pub const MODES: [Mode; 8] = [
    mode(3, 4, 0, 0, 4, 0, 1, 0, 3, 0),
    mode(2, 6, 0, 0, 6, 0, 0, 1, 3, 0),
    mode(3, 6, 0, 0, 5, 0, 0, 0, 2, 0),
    mode(2, 6, 0, 0, 7, 0, 1, 0, 2, 0),
    mode(1, 0, 2, 1, 5, 6, 0, 0, 2, 3),
    mode(1, 0, 2, 0, 7, 8, 0, 0, 2, 2),
    mode(1, 0, 0, 0, 7, 7, 1, 0, 4, 0),
    mode(2, 6, 0, 0, 5, 5, 1, 0, 2, 0),
];

fn weights(bits: u32) -> &'static [u32] {
    match bits {
        2 => &[0, 21, 43, 64],
        3 => &[0, 9, 18, 27, 37, 46, 55, 64],
        _ => &[0, 4, 9, 13, 17, 21, 26, 30, 34, 38, 43, 47, 51, 55, 60, 64],
    }
}

/// Sous-ensemble de chaque texel (ordre ligne par ligne), un texte par numéro de partition.
const PARTITIONS_2: [&str; 64] = [
    "0011001100110011", "0001000100010001", "0111011101110111", "0001001100110111",
    "0000000100010011", "0011011101111111", "0001001101111111", "0000000100110111",
    "0000000000010011", "0011011111111111", "0000000101111111", "0000000000010111",
    "0001011111111111", "0000000011111111", "0000111111111111", "0000000000001111",
    "0000100011101111", "0111000100000000", "0000000010001110", "0111001100010000",
    "0011000100000000", "0000100011001110", "0000000010001100", "0111001100110001",
    "0011000100010000", "0000100010001100", "0110011001100110", "0011011001101100",
    "0001011111101000", "0000111111110000", "0111000110001110", "0011100110011100",
    "0101010101010101", "0000111100001111", "0101101001011010", "0011001111001100",
    "0011110000111100", "0101010110101010", "0110100101101001", "0101101010100101",
    "0111001111001110", "0001001111001000", "0011001001001100", "0011101111011100",
    "0110100110010110", "0011110011000011", "0110011010011001", "0000011001100000",
    "0100111001000000", "0010011100100000", "0000001001110010", "0000010011100100",
    "0110110010010011", "0011011011001001", "0110001110011100", "0011100111000110",
    "0110110011001001", "0110001100111001", "0111111010000001", "0001100011100111",
    "0000111100110011", "0011001111110000", "0010001011101110", "0100010001110111",
];
const PARTITIONS_3: [&str; 64] = [
    "0011001102212222", "0001001122112221", "0000200122112211", "0222002200110111",
    "0000000011221122", "0011001100220022", "0022002211111111", "0011001122112211",
    "0000000011112222", "0000111111112222", "0000111122222222", "0012001200120012",
    "0112011201120112", "0122012201220122", "0011011211221222", "0011200122002220",
    "0001001101121122", "0111001120012200", "0000112211221122", "0022002200221111",
    "0111011102220222", "0001000122212221", "0000001101220122", "0000110022102210",
    "0122012200110000", "0012001211222222", "0110122112210110", "0000011012211221",
    "0022110211020022", "0110011020022222", "0011012201220011", "0000200022112221",
    "0000000211221222", "0222002200120011", "0011001200220222", "0120012001200120",
    "0000111122220000", "0120120120120120", "0120201212010120", "0011220011220011",
    "0011112222000011", "0101010122222222", "0000000021212121", "0022112200221122",
    "0022001100220011", "0220122102201221", "0101222222220101", "0000212121212121",
    "0101010101012222", "0222011102220111", "0002111200021112", "0000211221122112",
    "0222011101110222", "0002111211120002", "0110011001102222", "0000000021122112",
    "0110011022222222", "0022001100110022", "0022112211220022", "0000000000002112",
    "0002000100020001", "0222122202221222", "0101222222222222", "0111201122012220",
];
/// Texel dont l'index perd son bit de poids fort dans le deuxième (et troisième) sous-ensemble.
const ANCHORS_2: [u8; 64] = [
    15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15,
    15, 2, 8, 2, 2, 8, 8, 15, 2, 8, 2, 2, 8, 8, 2, 2,
    15, 15, 6, 8, 2, 8, 15, 15, 2, 8, 2, 2, 2, 15, 15, 6,
    6, 2, 6, 8, 15, 15, 2, 2, 15, 15, 15, 15, 15, 2, 2, 15,
];
const ANCHORS_3: [[u8; 64]; 2] = [
    [3, 3, 15, 15, 8, 3, 15, 15, 8, 8, 6, 6, 6, 5, 3, 3,
     3, 3, 8, 15, 3, 3, 6, 10, 5, 8, 8, 6, 8, 5, 15, 15,
     8, 15, 3, 5, 6, 10, 8, 15, 15, 3, 15, 5, 15, 15, 15, 15,
     3, 15, 5, 5, 5, 8, 5, 10, 5, 10, 8, 13, 15, 12, 3, 3],
    [15, 8, 8, 3, 15, 15, 3, 8, 15, 15, 15, 15, 15, 15, 15, 8,
     15, 8, 15, 3, 15, 8, 15, 8, 3, 15, 6, 10, 15, 15, 10, 8,
     15, 3, 15, 10, 10, 8, 9, 10, 6, 15, 8, 15, 3, 6, 6, 8,
     15, 3, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 3, 15, 15, 8],
];

/// Sous-ensemble de chacun des 16 texels.
pub fn subsets(mode: usize, partition: usize) -> [u8; TEXELS] {
    let table = match MODES[mode].subsets {
        1 => return [0; TEXELS],
        2 => &PARTITIONS_2,
        _ => &PARTITIONS_3,
    };
    let mut out = [0u8; TEXELS];
    for (texel, digit) in table[partition].bytes().enumerate() {
        out[texel] = digit - b'0';
    }
    out
}

/// Texels d'ancrage (index raccourci d'un bit), le premier étant toujours le texel 0.
pub fn anchors(mode: usize, partition: usize) -> Vec<usize> {
    let count = MODES[mode].subsets;
    anchor_texels(count, partition)[..count as usize].to_vec()
}

fn anchor_texels(subsets: u32, partition: usize) -> [usize; 3] {
    match subsets {
        1 => [0; 3],
        2 => [0, ANCHORS_2[partition] as usize, 0],
        _ => [0, ANCHORS_3[0][partition] as usize, ANCHORS_3[1][partition] as usize],
    }
}

/// (nom, premier bit, largeur) de chaque champ ; le bit 0 est le bit faible du premier octet.
pub fn layout(mode: usize, partition: usize) -> Result<Vec<(String, u32, u32)>, String> {
    let m = *MODES.get(mode).ok_or("mode BC7 hors de 0..7")?;
    if partition >= 1 << m.partition_bits {
        return Err("partition hors limites pour ce mode".into());
    }
    let mut widths = vec![("mode".to_string(), mode as u32 + 1)];
    for (name, width) in [("partition", m.partition_bits), ("rotation", m.rotation_bits),
                          ("index_selection", m.selection_bits)] {
        if width > 0 {
            widths.push((name.to_string(), width));
        }
    }
    for (channel, width) in [("r", m.colour_bits), ("g", m.colour_bits), ("b", m.colour_bits), ("a", m.alpha_bits)] {
        if width > 0 {
            widths.extend((0..2 * m.subsets).map(|endpoint| (format!("{channel}{endpoint}"), width)));
        }
    }
    widths.extend((0..2 * m.subsets * m.endpoint_p + m.subsets * m.subset_p).map(|i| (format!("p{i}"), 1)));
    let short = anchors(mode, partition);
    widths.extend((0..TEXELS).map(|t| (format!("index{t}"), m.index_bits - short.contains(&t) as u32)));
    if m.index2_bits > 0 {
        widths.extend((0..TEXELS).map(|t| (format!("index2_{t}"), m.index2_bits - (t == 0) as u32)));
    }
    let mut spans = Vec::with_capacity(widths.len());
    let mut offset = 0;
    for (name, width) in widths {
        spans.push((name, offset, width));
        offset += width;
    }
    debug_assert_eq!(offset, BLOCK_BITS);
    Ok(spans)
}

pub type Fields = BTreeMap<String, u32>;

/// Champs nommés d'un bloc ; le champ `mode` contient le numéro du mode.
pub fn unpack(block: &[u8]) -> Result<Fields, String> {
    let bytes: [u8; BLOCK_BYTES] = block.try_into().map_err(|_| "un bloc BC7 fait 16 octets")?;
    if bytes[0] == 0 {
        return Err("mode réservé".into());
    }
    let bits = u128::from_le_bytes(bytes);
    let mode = bits.trailing_zeros() as usize;
    let partition = ((bits >> (mode + 1)) & ((1u128 << MODES[mode].partition_bits) - 1)) as usize;
    let mut fields: Fields = layout(mode, partition)?
        .into_iter()
        .map(|(name, offset, width)| (name, ((bits >> offset) & ((1u128 << width) - 1)) as u32))
        .collect();
    fields.insert("mode".into(), mode as u32);
    Ok(fields)
}

/// Bloc de 16 octets à partir d'un jeu complet de champs dans leurs bornes.
pub fn pack(fields: &Fields) -> Result<[u8; BLOCK_BYTES], String> {
    let mode = *fields.get("mode").ok_or("champ mode absent")? as usize;
    if mode >= MODES.len() {
        return Err("mode BC7 hors de 0..7".into());
    }
    let spans = layout(mode, *fields.get("partition").unwrap_or(&0) as usize)?;
    if spans.len() != fields.len() || spans.iter().any(|(name, _, _)| !fields.contains_key(name)) {
        return Err("les champs ne correspondent pas au mode".into());
    }
    let mut bits = 1u128 << mode;
    for (name, offset, width) in &spans[1..] {
        let value = fields[name];
        if value >= 1 << width {
            return Err(format!("{name} dépasse {width} bits"));
        }
        bits |= (value as u128) << offset;
    }
    Ok(bits.to_le_bytes())
}

struct Bits {
    value: u128,
    position: u32,
}

impl Bits {
    fn take(&mut self, width: u32) -> u32 {
        let value = ((self.value >> self.position) & ((1u128 << width) - 1)) as u32;
        self.position += width;
        value
    }
}

/// Décode un bloc en 16 texels RGBA8, ligne par ligne ; un mode réservé donne des zéros.
pub fn decode(block: &[u8]) -> Result<[u8; RGBA_BYTES], String> {
    let bytes: [u8; BLOCK_BYTES] = block.try_into().map_err(|_| "un bloc BC7 fait 16 octets")?;
    if bytes[0] == 0 {
        return Ok([0; RGBA_BYTES]);
    }
    let value = u128::from_le_bytes(bytes);
    let index = value.trailing_zeros() as usize;
    let m = MODES[index];
    let mut bits = Bits { value, position: index as u32 + 1 };
    let partition = bits.take(m.partition_bits) as usize;
    let rotation = bits.take(m.rotation_bits) as usize;
    let selection = bits.take(m.selection_bits);
    let endpoints = endpoints(&m, &mut bits);
    let anchors = anchor_texels(m.subsets, partition).into_iter().fold(0u16, |mask, texel| mask | 1 << texel);
    let mut primary_index = [0u32; TEXELS];
    for (texel, slot) in primary_index.iter_mut().enumerate() {
        *slot = bits.take(m.index_bits - u32::from((anchors >> texel) & 1));
    }
    let mut secondary_index = [0u32; TEXELS];
    if m.index2_bits > 0 {
        for (texel, slot) in secondary_index.iter_mut().enumerate() {
            *slot = bits.take(m.index2_bits - (texel == 0) as u32);
        }
    }
    let primary_weights = weights(m.index_bits);
    let (colour_index, colour_weights, alpha_index, alpha_weights) = match (m.index2_bits, selection) {
        (0, _) => (&primary_index, primary_weights, &primary_index, primary_weights),
        (width, 0) => (&primary_index, primary_weights, &secondary_index, weights(width)),
        (width, _) => (&secondary_index, weights(width), &primary_index, primary_weights),
    };
    let shape = subsets(index, partition);
    let mut pixels = [0u8; RGBA_BYTES];
    for texel in 0..TEXELS {
        let cw = colour_weights[colour_index[texel] as usize];
        let aw = alpha_weights[alpha_index[texel] as usize];
        let subset = 2 * shape[texel] as usize;
        let (low, high) = (endpoints[subset], endpoints[subset + 1]);
        let mut colour = [0u32; 4];
        for channel in 0..4 {
            let w = if channel == 3 { aw } else { cw };
            colour[channel] = ((64 - w) * low[channel] + w * high[channel] + 32) >> 6;
        }
        if rotation > 0 {
            colour.swap(rotation - 1, 3);
        }
        for channel in 0..4 {
            pixels[texel * 4 + channel] = colour[channel] as u8;
        }
    }
    Ok(pixels)
}

/// Extrémités RGBA8 : lecture, bits P, puis réplication des bits forts dans les bits faibles.
fn endpoints(m: &Mode, bits: &mut Bits) -> [[u32; 4]; 6] {
    let count = 2 * m.subsets as usize;
    let mut raw = [[0u32; 4]; 6];
    for channel in 0..4 {
        let width = if channel == 3 { m.alpha_bits } else { m.colour_bits };
        if width > 0 {
            for endpoint in raw.iter_mut().take(count) {
                endpoint[channel] = bits.take(width);
            }
        }
    }
    let mut p = [0u32; 6];
    for slot in p.iter_mut().take((2 * m.subsets * m.endpoint_p + m.subsets * m.subset_p) as usize) {
        *slot = bits.take(1);
    }
    let mut out = [[0u32; 4]; 6];
    for endpoint in 0..count {
        for channel in 0..4 {
            let mut width = if channel == 3 { m.alpha_bits } else { m.colour_bits };
            if width == 0 {
                out[endpoint][channel] = 255;
                continue;
            }
            let mut value = raw[endpoint][channel];
            if m.endpoint_p + m.subset_p > 0 {
                value = value << 1 | p[if m.endpoint_p > 0 { endpoint } else { endpoint / 2 }];
                width += 1;
            }
            out[endpoint][channel] = ((value << (8 - width)) | (value >> (2 * width - 8))) & 255;
        }
    }
    out
}
