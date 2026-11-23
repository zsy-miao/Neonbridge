//! Delta color compression of 256-byte color blocks. A raw block (key 0xff)
//! is a valid hardware encoding even when the input could compress further.
use core::ptr;
use windows_sys::Win32::Foundation::{E_INVALIDARG, E_NOTIMPL, S_OK};

const BLOCK: usize = 256;
const SECTOR: usize = 64;
type Block = [u8; BLOCK];

// Morton-order neighbor obtained by subtracting one from the coordinate
// containing the least significant set bit of the interleaved pixel index.
const PARENT: [usize; BLOCK] = {
    let mut table = [0; BLOCK];
    let mut i = 1usize;
    while i < BLOCK {
        let bit = 1usize << i.trailing_zeros();
        let axis = if i.trailing_zeros().is_multiple_of(2) {
            0x55
        } else {
            0xaa
        };
        table[i] = i - bit + ((bit - 1) & axis);
        i += 1;
    }
    table
};

struct SectorEncoding {
    header: [u8; 2],
    bases: [u8; 8],
    base_count: usize,
    planes: [u8; 64],
    plane_count: usize,
}

struct Profile {
    channels: usize,
    decorrelate: bool,
    component_bytes: usize,
    one: u32,
    // Packed formats store their RGB and alpha clear masks in a single word.
    packed_clear: Option<(u32, u32)>,
    compact: bool,
    sector_span: usize,
}

impl Profile {
    // Each group stores two header bytes per sector, followed by the nonzero
    // channel bases in sector order. Four-channel color can use G, R-G, B-G, A.
    fn bases(&self, pixel: &[u8]) -> [u8; 4] {
        let mut bases = [0; 4];
        bases[..self.channels].copy_from_slice(pixel);
        if self.decorrelate {
            bases[0] = pixel[1];
            bases[1] = pixel[0].wrapping_sub(pixel[1]);
            bases[2] = pixel[2].wrapping_sub(pixel[1]);
        }
        bases
    }

    fn encode_sector(&self, src: &[u8]) -> SectorEncoding {
        let mut part = SectorEncoding {
            header: [0; 2],
            bases: [0; 8],
            base_count: 0,
            planes: [0; 64],
            plane_count: 0,
        };
        let mut values = [0; SECTOR];
        for (pixel, dest) in src
            .chunks_exact(self.channels)
            .zip(values.chunks_exact_mut(self.channels))
        {
            dest.copy_from_slice(&self.bases(pixel)[..self.channels]);
        }
        let per_channel = 8 / self.channels;
        for group in 0..8 {
            let channel = group / per_channel;
            let first = (group % per_channel) * 8;
            let value = |pixel: usize| values[pixel * self.channels + channel];
            let predicted = if first == 0 { 0 } else { value(PARENT[first]) };
            let base = value(first);
            let mut residuals = [0u8; 8];
            let mut high_bit = 0;
            for (i, r) in residuals.iter_mut().enumerate().skip(1) {
                let pixel = first + i;
                let diff = value(pixel).wrapping_sub(value(PARENT[pixel])) as i8;
                *r = ((diff << 1) ^ (diff >> 7)) as u8;
                high_bit |= *r;
            }
            let marker = if high_bit == 0 {
                part.header[1] |= 1 << group;
                if base == predicted && (first != 0 || self.sector_span == 1) {
                    continue;
                }
                base
            } else {
                let width = (8 - high_bit.leading_zeros()) as usize;
                let mask = ((1u16 << width) - 1) as u8;
                residuals[0] = base & mask;
                for bit in 0..width {
                    let plane = residuals
                        .iter()
                        .enumerate()
                        .fold(0, |v, (i, r)| v | (((r >> bit) & 1) << i));
                    part.planes[part.plane_count] = plane;
                    part.plane_count += 1;
                }
                (base & !mask) | (1 << (width - 1))
            };
            part.header[0] |= 1 << group;
            part.bases[part.base_count] = marker;
            part.base_count += 1;
        }
        part
    }

    fn encode(&self, src: &Block) -> (Block, u8, u32) {
        if !self.compact {
            let pixel_bytes = self.channels * self.component_bytes;
            let pixel = &src[..pixel_bytes];
            if src.chunks_exact(pixel_bytes).all(|p| p == pixel) {
                let mut packed = [0; BLOCK];
                packed[..pixel_bytes].copy_from_slice(pixel);
                return (packed, 0x10, 32);
            }
            return (*src, 0xff, BLOCK as u32);
        }
        let parts: [_; 4] =
            core::array::from_fn(|i| self.encode_sector(&src[i * SECTOR..(i + 1) * SECTOR]));
        let mut costs = [0usize; 5];
        let mut groups = [(1, 3u8); 4];
        for first in (0..4).rev() {
            costs[first] = SECTOR + costs[first + 1];
            let end = (first / self.channels + 1) * self.channels;
            let mut size = 0;
            for stop in first + 1..=end {
                let part = &parts[stop - 1];
                size += 2 + part.base_count + part.plane_count;
                let sectors = stop - first;
                let chunks = size.div_ceil(32);
                let key = match (sectors, chunks) {
                    (_, 1) => 1 << (sectors - 1),
                    (2, 2) => 0x06,
                    (3, 2) => 0x0c,
                    (4, 2) => 0x28,
                    _ => continue,
                };
                let cost = 32 * chunks + costs[stop];
                if cost <= costs[first] {
                    costs[first] = cost;
                    groups[first] = (sectors, key);
                }
            }
        }
        let mut packed = [0; BLOCK];
        let mut key = 0;
        let mut first = 0;
        while first < 4 {
            let (sectors, field) = groups[first];
            key |= field << (first * 2);
            let mut pos = first * SECTOR;
            if field == 3 {
                packed[pos..pos + SECTOR].copy_from_slice(&src[pos..pos + SECTOR]);
            } else {
                let group = &parts[first..first + sectors];
                for part in group {
                    packed[pos..pos + 2].copy_from_slice(&part.header);
                    pos += 2;
                }
                for part in group {
                    packed[pos..pos + part.base_count]
                        .copy_from_slice(&part.bases[..part.base_count]);
                    pos += part.base_count;
                }
                for part in group {
                    packed[pos..pos + part.plane_count]
                        .copy_from_slice(&part.planes[..part.plane_count]);
                    pos += part.plane_count;
                }
            }
            first += sectors;
        }
        (packed, key, costs[0] as u32)
    }

    // Each group uses two key bits per 64-byte sector. A single sector with
    // code 3 is raw. Runs of 1..4 sectors with just the stop bit set occupy
    // one 32-byte chunk; the channel headers determine how to expand it.
    fn groups(&self, key: u8, mut visit: impl FnMut(usize, usize, bool, usize) -> bool) -> bool {
        let mut first = 0;
        while first < 4 {
            let bits = u32::from(key) >> (2 * first);
            let sectors = bits.trailing_zeros() as usize + 1;
            if sectors > self.channels * self.sector_span || first + sectors > 4 {
                return false;
            }
            let code = bits & ((1 << (2 * sectors)) - 1);
            let raw = code == 3;
            let marker = 1 << (sectors - 1);
            let stride = [0, 2, 4, 8, 32][sectors];
            if !(code - marker).is_multiple_of(stride) {
                return false;
            }
            let capacity = 32 * (1 + (code - marker) / stride) as usize;
            if capacity > sectors * SECTOR {
                return false;
            }
            if !visit(first * SECTOR, sectors, raw, capacity) {
                return false;
            }
            first += sectors;
        }
        true
    }

    // Within a 64-byte sector, every channel is divided into groups of eight
    // texels in Morton order. The first byte selects new bases, the second
    // selects groups whose prediction has no residual. All bases precede the
    // transposed residual bit planes in the compressed group.
    fn compressed(&self, src: &Block, key: u8, out: Option<&mut Block>) -> bool {
        if !self.compact {
            return false;
        }
        let mut values = [0u8; BLOCK];
        let mut channel_widths = [[0usize; 4]; 4];
        let ok = self.groups(key, |offset, sectors, raw, capacity| {
            if raw {
                if out.is_some() {
                    for (pixel, dst) in src[offset..offset + SECTOR]
                        .chunks_exact(self.channels)
                        .zip(values[offset..offset + SECTOR].chunks_exact_mut(self.channels))
                    {
                        dst.copy_from_slice(&self.bases(pixel)[..self.channels]);
                    }
                }
                return true;
            }
            let group = &src[offset..offset + capacity];
            let (headers, stream) = group.split_at(2 * sectors);
            let (headers, _) = headers.as_chunks::<2>();
            let base_count = headers.iter().map(|h| h[0].count_ones() as usize).sum();
            let Some((bases, mut planes)) = stream.split_at_checked(base_count) else {
                return false;
            };
            let mut bases = bases.iter().copied();
            for (sector, header) in headers.iter().enumerate() {
                let start = offset + sector * SECTOR;
                let origin = start / (SECTOR * self.sector_span) * (SECTOR * self.sector_span);
                let mut width = 0usize;
                let per_channel = 8 / self.channels;
                for group in 0..8 {
                    let reset = header[0] & (1 << group) != 0;
                    let delta = header[1] & (1 << group) == 0;
                    let channel = group / per_channel;
                    let first = (start - origin) / self.channels + (group % per_channel) * 8;
                    let channel_start = group % per_channel == 0;
                    // The first group for a channel inherits from the same
                    // channel of the previous sector inside the prediction window.
                    if channel_start && start != origin {
                        width = channel_widths[start / SECTOR - 1][channel];
                    }
                    let mut base = 0;
                    if reset {
                        base = bases.next().unwrap();
                        if delta {
                            if base == 0 {
                                return false;
                            }
                            width = base.trailing_zeros() as usize + 1;
                            base &= !((1u16 << width) - 1) as u8;
                        }
                    }
                    let count = if delta { width } else { 0 };
                    if channel_start {
                        channel_widths[start / SECTOR][channel] = count;
                    }
                    if delta && count == 0 {
                        return false;
                    }
                    let Some((bits, rest)) = planes.split_at_checked(count) else {
                        return false;
                    };
                    planes = rest;
                    if out.is_none() {
                        continue;
                    }
                    for i in 0..8 {
                        let pixel = first + i;
                        let code = bits
                            .iter()
                            .enumerate()
                            .fold(0u8, |v, (b, plane)| v | (((plane >> i) & 1) << b));
                        let value = if i == 0 && reset {
                            base | code
                        } else {
                            let parent = values[origin + PARENT[pixel] * self.channels + channel];
                            let residual = (code >> 1) ^ 0u8.wrapping_sub(code & 1);
                            parent.wrapping_add(residual)
                        };
                        values[origin + pixel * self.channels + channel] = value;
                    }
                }
            }
            true
        });
        if ok && let Some(dst) = out {
            if self.decorrelate {
                for pixel in values.as_chunks_mut::<4>().0 {
                    let green = pixel[0];
                    pixel[0] = pixel[1].wrapping_add(green);
                    pixel[1] = green;
                    pixel[2] = pixel[2].wrapping_add(green);
                }
            }
            *dst = values;
        }
        ok
    }

    fn supports(&self, src: &Block, key: u8) -> bool {
        matches!(key, 0x00 | 0x10 | 0x40 | 0x80 | 0xc0 | 0xff) || self.compressed(src, key, None)
    }

    fn decode(&self, src: &Block, key: u8) -> Block {
        // Fast clears carry either one stored texel or channel-wise zero/one.
        if matches!(key, 0x00 | 0x10 | 0x40 | 0x80 | 0xc0) {
            let bytes = self.channels * self.component_bytes;
            let mut pixel = [0; 16];
            if key == 0x10 {
                pixel[..bytes].copy_from_slice(&src[..bytes]);
            } else if let Some((rgb, alpha)) = self.packed_clear {
                let value = (if key & 0x80 != 0 { rgb } else { 0 })
                    | (if key & 0x40 != 0 { alpha } else { 0 });
                pixel[..bytes].copy_from_slice(&value.to_le_bytes()[..bytes]);
            } else {
                for (i, dst) in pixel[..bytes]
                    .chunks_exact_mut(self.component_bytes)
                    .enumerate()
                {
                    let bit = if i + 1 == self.channels { 0x40 } else { 0x80 };
                    if key & bit != 0 {
                        dst.copy_from_slice(&self.one.to_le_bytes()[..self.component_bytes]);
                    }
                }
            }
            let mut raw = [0; BLOCK];
            for dst in raw.chunks_exact_mut(bytes) {
                dst.copy_from_slice(&pixel[..bytes]);
            }
            return raw;
        }
        let mut raw = [0; BLOCK];
        self.compressed(src, key, Some(&mut raw));
        raw
    }
}

unsafe fn profile(
    format: u32,
    swizzle: u32,
    dimension: u32,
    settings: *const u32,
) -> Result<Profile, i32> {
    if settings.is_null() || !(1..=120).contains(&format) || swizzle > 31 {
        return Err(E_INVALIDARG);
    }
    // SAFETY: The guest supplies the two DWORDs of DCC configuration.
    let [block_size, independent_channels] =
        unsafe { settings.cast::<[u32; 2]>().read_unaligned() };
    if block_size > 2
        || independent_channels > 1
        || !swizzle.is_multiple_of(4)
        || !matches!(dimension, 0 | 3 | 4)
    {
        return Err(E_NOTIMPL);
    }
    let (channels, component_bytes) = match format {
        2..=4 => (4, 4),
        10..=14 => (4, 2),
        16..=18 => (2, 4),
        28..=32 | 87 | 88 | 91 | 93 => (4, 1),
        34..=38 => (2, 2),
        24..=26 | 41..=43 => (1, 4),
        49..=52 => (2, 1),
        54 | 56..=59 => (1, 2),
        61..=65 => (1, 1),
        _ => return Err(E_NOTIMPL),
    };
    let one = match format {
        2 | 16 | 41 => 1.0f32.to_bits(),
        10 | 34 | 54 => 0x3c00,
        4 | 18 | 43 | 13 | 14 | 37 | 38 | 58 | 59 | 31 | 32 | 51 | 52 | 63 | 64 => {
            (1u32 << (8 * component_bytes - 1)) - 1
        }
        _ => u32::MAX,
    };
    Ok(Profile {
        channels,
        component_bytes,
        one,
        packed_clear: match format {
            // The Scarlett codec clears both 10:10:10:2 formats to zero.
            24 | 25 => Some((0, 0)),
            26 => Some((0x781e03c0, 0)),
            65 => Some((0xff, 0)),
            _ => None,
        },
        compact: component_bytes == 1,
        sector_span: 1usize << block_size,
        decorrelate: channels == 4 && independent_channels == 0,
    })
}

/// # Safety
/// The guest supplies count 256-byte blocks, count keys and optional DWORD
/// sizes. Blocks may be transformed in place.
#[unsafe(export_name = "XGEncodeDCC")]
unsafe extern "system" fn encode(
    count: u32,
    src: *const Block,
    dst: *mut Block,
    keys: *mut u8,
    sizes: *mut u32,
    format: u32,
    swizzle: u32,
    dimension: u32,
    settings: *const u32,
) -> i32 {
    if src.is_null() || dst.is_null() || keys.is_null() {
        return E_INVALIDARG;
    }
    let profile = match unsafe { profile(format, swizzle, dimension, settings) } {
        Ok(p) => p,
        Err(hr) => return hr,
    };
    for i in 0..count as usize {
        let raw = unsafe { src.add(i).read() };
        let (packed, key, size) = profile.encode(&raw);
        unsafe {
            dst.add(i).write(packed);
            keys.add(i).write(key);
            if !sizes.is_null() {
                sizes.add(i).write_unaligned(size);
            }
        }
    }
    S_OK
}

/// # Safety
/// The guest supplies count readable blocks and keys and count writable blocks.
#[unsafe(export_name = "XGDecodeDCC")]
unsafe extern "system" fn decode(
    count: u32,
    src: *const Block,
    keys: *const u8,
    dst: *mut Block,
    format: u32,
    swizzle: u32,
    dimension: u32,
    settings: *const u32,
) -> i32 {
    if src.is_null() || dst.is_null() || keys.is_null() {
        return E_INVALIDARG;
    }
    let profile = match unsafe { profile(format, swizzle, dimension, settings) } {
        Ok(p) => p,
        Err(hr) => return hr,
    };
    // Validate every key and header before publishing any of the batch.
    for i in 0..count as usize {
        if !profile.supports(unsafe { &*src.add(i) }, unsafe { keys.add(i).read() }) {
            return E_NOTIMPL;
        }
    }
    for i in 0..count as usize {
        let key = unsafe { keys.add(i).read() };
        if key == 0xff {
            unsafe {
                ptr::copy(src.add(i), dst.add(i), 1);
            }
        } else {
            let raw = profile.decode(unsafe { &*src.add(i) }, key);
            unsafe {
                dst.add(i).write(raw);
            }
        }
    }
    S_OK
}
