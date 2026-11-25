//! The LLVM 3.7 bitstream DXIL is written in: little-endian 32-bit words filled
//! from bit 0, blocks with a length word, unabbreviated records (code, operand
//! count and operands as 6-bit VBR) or records in a block's own abbreviations.
//!
//! Ported from five-star-recomp (GPL-3.0-or-later, ThatRecompDev and the Five
//! Star Recomp contributors), src/FiveStar.Shaders/Dxil/Bitcode.

/// Abbreviation operand encodings (LLVM BitCodes.h: 1 fixed, 2 VBR, 3 array);
/// a literal is flagged apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Literal(u64),
    Fixed(u32),
    Vbr(u32),
    Array,
}

/// LLVM 3.7 block ids (LLVMBitCodes.h).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum BlockId {
    Module = 8,
    ParameterAttributes = 9,
    ParameterAttributeGroups = 10,
    Constants = 11,
    Function = 12,
    ValueSymbolTable = 14,
    Metadata = 15,
    Types = 17,
}

const TOP_WIDTH: u32 = 2;
const END_BLOCK: u64 = 0;
const ENTER_SUBBLOCK: u64 = 1;
const DEFINE_ABBREVIATION: u64 = 2;
const UNABBREVIATED_RECORD: u64 = 3;
/// The first abbreviation id a block defines.
pub const FIRST_ABBREVIATION: u32 = 4;

struct Block {
    width: u32,
    /// Word index just past the block's length word.
    start: usize,
    abbreviations: Vec<Vec<Op>>,
}

#[derive(Default)]
pub struct Writer {
    words: Vec<u32>,
    blocks: Vec<Block>,
    current: u64,
    bits: u32,
}

impl Writer {
    pub fn new() -> Self {
        Self::default()
    }

    fn width(&self) -> u32 {
        self.blocks.last().map_or(TOP_WIDTH, |b| b.width)
    }

    /// Raw bytes (the magic number).
    pub fn bytes(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.emit(u64::from(b), 8);
        }
    }

    /// Opens a block whose abbreviation ids are `width` bits wide.
    pub fn enter(&mut self, id: BlockId, width: u32) {
        self.emit(ENTER_SUBBLOCK, self.width());
        self.vbr(id as u64, 8);
        self.vbr(u64::from(width), 4);
        self.align();
        self.words.push(0);
        self.blocks.push(Block { width, start: self.words.len(), abbreviations: Vec::new() });
    }

    /// Closes the innermost block and fills in its length.
    pub fn exit(&mut self) {
        self.emit(END_BLOCK, self.width());
        self.align();
        let block = self.blocks.pop().expect("exit without a block");
        self.words[block.start - 1] = (self.words.len() - block.start) as u32;
    }

    /// One unabbreviated record.
    pub fn record(&mut self, code: u32, operands: &[u64]) {
        self.emit(UNABBREVIATED_RECORD, self.width());
        self.vbr(u64::from(code), 6);
        self.vbr(operands.len() as u64, 6);
        for &o in operands {
            self.vbr(o, 6);
        }
    }

    /// Defines an abbreviation in the current block (its first operand is the
    /// record code, a literal); returns its id.
    pub fn define_abbreviation(&mut self, operands: &[Op]) -> u32 {
        self.emit(DEFINE_ABBREVIATION, self.width());
        self.vbr(operands.len() as u64, 5);
        for &op in operands {
            match op {
                Op::Literal(v) => {
                    self.emit(1, 1);
                    self.vbr(v, 8);
                }
                Op::Fixed(w) => {
                    self.emit(0, 1);
                    self.emit(1, 3);
                    self.vbr(u64::from(w), 5);
                }
                Op::Vbr(w) => {
                    self.emit(0, 1);
                    self.emit(2, 3);
                    self.vbr(u64::from(w), 5);
                }
                Op::Array => {
                    self.emit(0, 1);
                    self.emit(3, 3);
                }
            }
        }
        let block = self.blocks.last_mut().expect("abbreviation outside a block");
        block.abbreviations.push(operands.to_vec());
        FIRST_ABBREVIATION + block.abbreviations.len() as u32 - 1
    }

    /// A record in abbreviation `id`: the operands after its literal code, in its fields.
    pub fn abbreviated(&mut self, id: u32, operands: &[u64]) {
        let abbreviation = self.blocks.last().expect("record outside a block").abbreviations
            [(id - FIRST_ABBREVIATION) as usize].clone();
        self.emit(u64::from(id), self.width());
        let mut next = 0;
        let mut index = 1;
        while index < abbreviation.len() {
            match abbreviation[index] {
                Op::Literal(_) => next += 1,
                Op::Array => {
                    let element = abbreviation[index + 1];
                    self.vbr((operands.len() - next) as u64, 6);
                    for &o in &operands[next..] {
                        self.field(element, o);
                    }
                    return;
                }
                op => {
                    self.field(op, operands[next]);
                    next += 1;
                }
            }
            index += 1;
        }
    }

    /// A record whose operands are `prefix` then the characters of `text`.
    pub fn text_record(&mut self, code: u32, prefix: &[u64], text: &str) {
        let mut operands = prefix.to_vec();
        operands.extend(text.bytes().map(u64::from));
        self.record(code, &operands);
    }

    /// The finished stream.
    pub fn finish(mut self) -> Vec<u8> {
        self.align();
        self.words.iter().flat_map(|w| w.to_le_bytes()).collect()
    }

    /// LLVM's signed VBR operand: the magnitude shifted left, the sign in bit 0.
    pub fn signed(value: i64) -> u64 {
        if value >= 0 { (value as u64) << 1 } else { (value.unsigned_abs() << 1) | 1 }
    }

    fn emit(&mut self, value: u64, width: u32) {
        if width == 0 {
            return;
        }
        // A field may straddle the word boundary: keep its high bits.
        let value = if width >= 64 { value } else { value & ((1u64 << width) - 1) };
        let mut value = value;
        let mut width = width;
        while width > 0 {
            let room = 32 - self.bits;
            let take = width.min(room);
            let part = if take >= 64 { value } else { value & ((1u64 << take) - 1) };
            self.current |= part << self.bits;
            self.bits += take;
            width -= take;
            value = if take >= 64 { 0 } else { value >> take };
            if self.bits == 32 {
                self.words.push(self.current as u32);
                self.current = 0;
                self.bits = 0;
            }
        }
    }

    fn field(&mut self, op: Op, value: u64) {
        match op {
            Op::Fixed(w) => self.emit(value, w),
            Op::Vbr(w) => self.vbr(value, w),
            other => unreachable!("field of kind {other:?}"),
        }
    }

    fn vbr(&mut self, mut value: u64, width: u32) {
        let high = 1u64 << (width - 1);
        while value >= high {
            self.emit((value & (high - 1)) | high, width);
            value >>= width - 1;
        }
        self.emit(value, width);
    }

    fn align(&mut self) {
        if self.bits > 0 {
            self.words.push(self.current as u32);
            self.current = 0;
            self.bits = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_length_counts_words_after_the_length_word() {
        let mut w = Writer::new();
        w.bytes(b"BC\xC0\xDE");
        w.enter(BlockId::Module, 3);
        w.record(1, &[1]);
        w.exit();
        let bytes = w.finish();
        assert_eq!(&bytes[..4], b"BC\xC0\xDE");
        let length = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
        assert_eq!(bytes.len(), 12 + length as usize * 4);
    }

    #[test]
    fn signed_vbr_puts_the_sign_in_bit_zero() {
        assert_eq!(Writer::signed(3), 6);
        assert_eq!(Writer::signed(-3), 7);
    }
}
