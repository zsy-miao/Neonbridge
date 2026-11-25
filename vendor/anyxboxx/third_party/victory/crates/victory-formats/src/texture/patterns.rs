//! Swizzle patterns of the console GPU (RB+, GFX10.3) for each mode and
//! element size, and for the pipe and bank XOR modes each pipe
//! configuration. Generated from Mesa's addrlib
//! (`src/amd/addrlib/src/gfx10/gfx10SwizzlePattern.h`) by
//! XTP's `texture/generate.rs` (MiamiGame/XTP, commit 00c60fb; the generator
//! is not carried here): do not edit. addrlib carries this notice:
//!
//! Copyright (C) 2007-2026 Advanced Micro Devices, Inc. All rights reserved.
//! SPDX-License-Identifier: MIT
//!
//! Permission is hereby granted, free of charge, to any person obtaining a
//! copy of this software and associated documentation files (the
//! "Software"), to deal in the Software without restriction, including
//! without limitation the rights to use, copy, modify, merge, publish,
//! distribute, sublicense, and/or sell copies of the Software, and to permit
//! persons to whom the Software is furnished to do so, subject to the
//! following conditions: the above copyright notice and this permission
//! notice shall be included in all copies or substantial portions of the
//! Software. THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
//! EXPRESS OR IMPLIED.

use super::{Pattern, Pipes, Swizzle, x, y, z};

pub(super) const PATTERNS: &[Pattern] = &[
    // GFX10_SW_256_S_RBPLUS_PATINFO[0]: 1 pipes (1 PKRs) 1 bpe @ SW_256_S @ RbPlus
    Pattern { swizzle: Swizzle(1), thick: false, pipes: None, element_log2: 0, bits: &[
        x(0), x(1), x(2), x(3), y(0), y(1), y(2), y(3)]
    },
    // GFX10_SW_256_S_RBPLUS_PATINFO[1]: 1 pipes (1 PKRs) 2 bpe @ SW_256_S @ RbPlus
    Pattern { swizzle: Swizzle(1), thick: false, pipes: None, element_log2: 1, bits: &[
        0, x(0), x(1), x(2), y(0), y(1), y(2), x(3)]
    },
    // GFX10_SW_256_S_RBPLUS_PATINFO[2]: 1 pipes (1 PKRs) 4 bpe @ SW_256_S @ RbPlus
    Pattern { swizzle: Swizzle(1), thick: false, pipes: None, element_log2: 2, bits: &[
        0, 0, x(0), x(1), y(0), y(1), y(2), x(2)]
    },
    // GFX10_SW_256_S_RBPLUS_PATINFO[3]: 1 pipes (1 PKRs) 8 bpe @ SW_256_S @ RbPlus
    Pattern { swizzle: Swizzle(1), thick: false, pipes: None, element_log2: 3, bits: &[
        0, 0, 0, x(0), y(0), y(1), x(1), x(2)]
    },
    // GFX10_SW_256_S_RBPLUS_PATINFO[4]: 1 pipes (1 PKRs) 16 bpe @ SW_256_S @ RbPlus
    Pattern { swizzle: Swizzle(1), thick: false, pipes: None, element_log2: 4, bits: &[
        0, 0, 0, 0, y(0), y(1), x(0), x(1)]
    },
    // GFX10_SW_256_D_RBPLUS_PATINFO[0]: 1 pipes (1 PKRs) 1 bpe @ SW_256_D @ RbPlus
    Pattern { swizzle: Swizzle(2), thick: false, pipes: None, element_log2: 0, bits: &[
        x(0), x(1), x(2), y(1), y(0), y(2), x(3), y(3)]
    },
    // GFX10_SW_256_D_RBPLUS_PATINFO[1]: 1 pipes (1 PKRs) 2 bpe @ SW_256_D @ RbPlus
    Pattern { swizzle: Swizzle(2), thick: false, pipes: None, element_log2: 1, bits: &[
        0, x(0), x(1), x(2), y(0), y(1), y(2), x(3)]
    },
    // GFX10_SW_256_D_RBPLUS_PATINFO[2]: 1 pipes (1 PKRs) 4 bpe @ SW_256_D @ RbPlus
    Pattern { swizzle: Swizzle(2), thick: false, pipes: None, element_log2: 2, bits: &[
        0, 0, x(0), x(1), y(0), y(1), x(2), y(2)]
    },
    // GFX10_SW_256_D_RBPLUS_PATINFO[3]: 1 pipes (1 PKRs) 8 bpe @ SW_256_D @ RbPlus
    Pattern { swizzle: Swizzle(2), thick: false, pipes: None, element_log2: 3, bits: &[
        0, 0, 0, x(0), y(0), x(1), x(2), y(1)]
    },
    // GFX10_SW_256_D_RBPLUS_PATINFO[4]: 1 pipes (1 PKRs) 16 bpe @ SW_256_D @ RbPlus
    Pattern { swizzle: Swizzle(2), thick: false, pipes: None, element_log2: 4, bits: &[
        0, 0, 0, 0, x(0), y(0), x(1), y(1)]
    },
    // GFX10_SW_4K_S_RBPLUS_PATINFO[0]: 1 pipes (1 PKRs) 1 bpe @ SW_4K_S @ RbPlus
    Pattern { swizzle: Swizzle(5), thick: false, pipes: None, element_log2: 0, bits: &[
        x(0), x(1), x(2), x(3), y(0), y(1), y(2), y(3), y(4), x(4), y(5), x(5)]
    },
    // GFX10_SW_4K_S_RBPLUS_PATINFO[1]: 1 pipes (1 PKRs) 2 bpe @ SW_4K_S @ RbPlus
    Pattern { swizzle: Swizzle(5), thick: false, pipes: None, element_log2: 1, bits: &[
        0, x(0), x(1), x(2), y(0), y(1), y(2), x(3), y(3), x(4), y(4), x(5)]
    },
    // GFX10_SW_4K_S_RBPLUS_PATINFO[2]: 1 pipes (1 PKRs) 4 bpe @ SW_4K_S @ RbPlus
    Pattern { swizzle: Swizzle(5), thick: false, pipes: None, element_log2: 2, bits: &[
        0, 0, x(0), x(1), y(0), y(1), y(2), x(2), y(3), x(3), y(4), x(4)]
    },
    // GFX10_SW_4K_S_RBPLUS_PATINFO[3]: 1 pipes (1 PKRs) 8 bpe @ SW_4K_S @ RbPlus
    Pattern { swizzle: Swizzle(5), thick: false, pipes: None, element_log2: 3, bits: &[
        0, 0, 0, x(0), y(0), y(1), x(1), x(2), y(2), x(3), y(3), x(4)]
    },
    // GFX10_SW_4K_S_RBPLUS_PATINFO[4]: 1 pipes (1 PKRs) 16 bpe @ SW_4K_S @ RbPlus
    Pattern { swizzle: Swizzle(5), thick: false, pipes: None, element_log2: 4, bits: &[
        0, 0, 0, 0, y(0), y(1), x(0), x(1), y(2), x(2), y(3), x(3)]
    },
    // GFX10_SW_4K_D_RBPLUS_PATINFO[0]: 1 pipes (1 PKRs) 1 bpe @ SW_4K_D @ RbPlus
    Pattern { swizzle: Swizzle(6), thick: false, pipes: None, element_log2: 0, bits: &[
        x(0), x(1), x(2), y(1), y(0), y(2), x(3), y(3), y(4), x(4), y(5), x(5)]
    },
    // GFX10_SW_4K_D_RBPLUS_PATINFO[1]: 1 pipes (1 PKRs) 2 bpe @ SW_4K_D @ RbPlus
    Pattern { swizzle: Swizzle(6), thick: false, pipes: None, element_log2: 1, bits: &[
        0, x(0), x(1), x(2), y(0), y(1), y(2), x(3), y(3), x(4), y(4), x(5)]
    },
    // GFX10_SW_4K_D_RBPLUS_PATINFO[2]: 1 pipes (1 PKRs) 4 bpe @ SW_4K_D @ RbPlus
    Pattern { swizzle: Swizzle(6), thick: false, pipes: None, element_log2: 2, bits: &[
        0, 0, x(0), x(1), y(0), y(1), x(2), y(2), y(3), x(3), y(4), x(4)]
    },
    // GFX10_SW_4K_D_RBPLUS_PATINFO[3]: 1 pipes (1 PKRs) 8 bpe @ SW_4K_D @ RbPlus
    Pattern { swizzle: Swizzle(6), thick: false, pipes: None, element_log2: 3, bits: &[
        0, 0, 0, x(0), y(0), x(1), x(2), y(1), y(2), x(3), y(3), x(4)]
    },
    // GFX10_SW_4K_D_RBPLUS_PATINFO[4]: 1 pipes (1 PKRs) 16 bpe @ SW_4K_D @ RbPlus
    Pattern { swizzle: Swizzle(6), thick: false, pipes: None, element_log2: 4, bits: &[
        0, 0, 0, 0, x(0), y(0), x(1), y(1), y(2), x(2), y(3), x(3)]
    },
    // GFX10_SW_64K_S_RBPLUS_PATINFO[0]: 1 pipes (1 PKRs) 1 bpe @ SW_64K_S @ RbPlus
    Pattern { swizzle: Swizzle(9), thick: false, pipes: None, element_log2: 0, bits: &[
        x(0), x(1), x(2), x(3), y(0), y(1), y(2), y(3), y(4), x(4), y(5), x(5), y(6), x(6), y(7), x(7)]
    },
    // GFX10_SW_64K_S_RBPLUS_PATINFO[1]: 1 pipes (1 PKRs) 2 bpe @ SW_64K_S @ RbPlus
    Pattern { swizzle: Swizzle(9), thick: false, pipes: None, element_log2: 1, bits: &[
        0, x(0), x(1), x(2), y(0), y(1), y(2), x(3), y(3), x(4), y(4), x(5), y(5), x(6), y(6), x(7)]
    },
    // GFX10_SW_64K_S_RBPLUS_PATINFO[2]: 1 pipes (1 PKRs) 4 bpe @ SW_64K_S @ RbPlus
    Pattern { swizzle: Swizzle(9), thick: false, pipes: None, element_log2: 2, bits: &[
        0, 0, x(0), x(1), y(0), y(1), y(2), x(2), y(3), x(3), y(4), x(4), y(5), x(5), y(6), x(6)]
    },
    // GFX10_SW_64K_S_RBPLUS_PATINFO[3]: 1 pipes (1 PKRs) 8 bpe @ SW_64K_S @ RbPlus
    Pattern { swizzle: Swizzle(9), thick: false, pipes: None, element_log2: 3, bits: &[
        0, 0, 0, x(0), y(0), y(1), x(1), x(2), y(2), x(3), y(3), x(4), y(4), x(5), y(5), x(6)]
    },
    // GFX10_SW_64K_S_RBPLUS_PATINFO[4]: 1 pipes (1 PKRs) 16 bpe @ SW_64K_S @ RbPlus
    Pattern { swizzle: Swizzle(9), thick: false, pipes: None, element_log2: 4, bits: &[
        0, 0, 0, 0, y(0), y(1), x(0), x(1), y(2), x(2), y(3), x(3), y(4), x(4), y(5), x(5)]
    },
    // GFX10_SW_64K_D_RBPLUS_PATINFO[0]: 1 pipes (1 PKRs) 1 bpe @ SW_64K_D @ RbPlus
    Pattern { swizzle: Swizzle(10), thick: false, pipes: None, element_log2: 0, bits: &[
        x(0), x(1), x(2), y(1), y(0), y(2), x(3), y(3), y(4), x(4), y(5), x(5), y(6), x(6), y(7), x(7)]
    },
    // GFX10_SW_64K_D_RBPLUS_PATINFO[1]: 1 pipes (1 PKRs) 2 bpe @ SW_64K_D @ RbPlus
    Pattern { swizzle: Swizzle(10), thick: false, pipes: None, element_log2: 1, bits: &[
        0, x(0), x(1), x(2), y(0), y(1), y(2), x(3), y(3), x(4), y(4), x(5), y(5), x(6), y(6), x(7)]
    },
    // GFX10_SW_64K_D_RBPLUS_PATINFO[2]: 1 pipes (1 PKRs) 4 bpe @ SW_64K_D @ RbPlus
    Pattern { swizzle: Swizzle(10), thick: false, pipes: None, element_log2: 2, bits: &[
        0, 0, x(0), x(1), y(0), y(1), x(2), y(2), y(3), x(3), y(4), x(4), y(5), x(5), y(6), x(6)]
    },
    // GFX10_SW_64K_D_RBPLUS_PATINFO[3]: 1 pipes (1 PKRs) 8 bpe @ SW_64K_D @ RbPlus
    Pattern { swizzle: Swizzle(10), thick: false, pipes: None, element_log2: 3, bits: &[
        0, 0, 0, x(0), y(0), x(1), x(2), y(1), y(2), x(3), y(3), x(4), y(4), x(5), y(5), x(6)]
    },
    // GFX10_SW_64K_D_RBPLUS_PATINFO[4]: 1 pipes (1 PKRs) 16 bpe @ SW_64K_D @ RbPlus
    Pattern { swizzle: Swizzle(10), thick: false, pipes: None, element_log2: 4, bits: &[
        0, 0, 0, 0, x(0), y(0), x(1), y(1), y(2), x(2), y(3), x(3), y(4), x(4), y(5), x(5)]
    },
    // GFX10_SW_4K_S3_RBPLUS_PATINFO[0]: 1 pipes (1 PKRs) 1 bpe @ SW_4K_S3 @ RbPlus
    Pattern { swizzle: Swizzle(5), thick: true, pipes: None, element_log2: 0, bits: &[
        x(0), x(1), z(0), y(0), z(1), y(1), x(2), z(2), y(2), x(3), z(3), y(3)]
    },
    // GFX10_SW_4K_S3_RBPLUS_PATINFO[1]: 1 pipes (1 PKRs) 2 bpe @ SW_4K_S3 @ RbPlus
    Pattern { swizzle: Swizzle(5), thick: true, pipes: None, element_log2: 1, bits: &[
        0, x(0), z(0), y(0), z(1), y(1), x(1), z(2), y(2), x(2), z(3), y(3)]
    },
    // GFX10_SW_4K_S3_RBPLUS_PATINFO[2]: 1 pipes (1 PKRs) 4 bpe @ SW_4K_S3 @ RbPlus
    Pattern { swizzle: Swizzle(5), thick: true, pipes: None, element_log2: 2, bits: &[
        0, 0, x(0), y(0), z(0), y(1), x(1), z(1), y(2), x(2), z(2), y(3)]
    },
    // GFX10_SW_4K_S3_RBPLUS_PATINFO[3]: 1 pipes (1 PKRs) 8 bpe @ SW_4K_S3 @ RbPlus
    Pattern { swizzle: Swizzle(5), thick: true, pipes: None, element_log2: 3, bits: &[
        0, 0, 0, x(0), z(0), y(0), x(1), z(1), y(1), x(2), z(2), y(2)]
    },
    // GFX10_SW_4K_S3_RBPLUS_PATINFO[4]: 1 pipes (1 PKRs) 16 bpe @ SW_4K_S3 @ RbPlus
    Pattern { swizzle: Swizzle(5), thick: true, pipes: None, element_log2: 4, bits: &[
        0, 0, 0, 0, z(0), y(0), x(0), z(1), y(1), x(1), z(2), y(2)]
    },
    // GFX10_SW_64K_S3_RBPLUS_PATINFO[0]: 1 pipes (1 PKRs) 1 bpe @ SW_64K_S3 @ RbPlus
    Pattern { swizzle: Swizzle(9), thick: true, pipes: None, element_log2: 0, bits: &[
        x(0), x(1), z(0), y(0), z(1), y(1), x(2), z(2), y(2), x(3), z(3), y(3), x(4), z(4), y(4), x(5)]
    },
    // GFX10_SW_64K_S3_RBPLUS_PATINFO[1]: 1 pipes (1 PKRs) 2 bpe @ SW_64K_S3 @ RbPlus
    Pattern { swizzle: Swizzle(9), thick: true, pipes: None, element_log2: 1, bits: &[
        0, x(0), z(0), y(0), z(1), y(1), x(1), z(2), y(2), x(2), z(3), y(3), x(3), z(4), y(4), x(4)]
    },
    // GFX10_SW_64K_S3_RBPLUS_PATINFO[2]: 1 pipes (1 PKRs) 4 bpe @ SW_64K_S3 @ RbPlus
    Pattern { swizzle: Swizzle(9), thick: true, pipes: None, element_log2: 2, bits: &[
        0, 0, x(0), y(0), z(0), y(1), x(1), z(1), y(2), x(2), z(2), y(3), x(3), z(3), y(4), x(4)]
    },
    // GFX10_SW_64K_S3_RBPLUS_PATINFO[3]: 1 pipes (1 PKRs) 8 bpe @ SW_64K_S3 @ RbPlus
    Pattern { swizzle: Swizzle(9), thick: true, pipes: None, element_log2: 3, bits: &[
        0, 0, 0, x(0), z(0), y(0), x(1), z(1), y(1), x(2), z(2), y(2), x(3), z(3), y(3), x(4)]
    },
    // GFX10_SW_64K_S3_RBPLUS_PATINFO[4]: 1 pipes (1 PKRs) 16 bpe @ SW_64K_S3 @ RbPlus
    Pattern { swizzle: Swizzle(9), thick: true, pipes: None, element_log2: 4, bits: &[
        0, 0, 0, 0, z(0), y(0), x(0), z(1), y(1), x(1), z(2), y(2), x(2), z(3), y(3), x(3)]
    },
    // GFX10_SW_64K_S_T_RBPLUS_PATINFO[45]: 32 pipes (8 PKRs) 1 bpe @ SW_64K_S_T @ RbPlus
    Pattern { swizzle: Swizzle(17), thick: false, pipes: Some(Pipes::SERIES_X), element_log2: 0, bits: &[
        x(0), x(1), x(2), x(3), y(0), y(1), y(2), y(3), y(4), x(4), y(5) | x(7), x(5) | y(7), x(6) | y(6), x(6), y(7), x(7)]
    },
    // GFX10_SW_64K_S_T_RBPLUS_PATINFO[46]: 32 pipes (8 PKRs) 2 bpe @ SW_64K_S_T @ RbPlus
    Pattern { swizzle: Swizzle(17), thick: false, pipes: Some(Pipes::SERIES_X), element_log2: 1, bits: &[
        0, x(0), x(1), x(2), y(0), y(1), y(2), x(3), y(3), x(4), y(4) | x(7), x(5) | y(6), y(5) | x(6), x(6), y(6), x(7)]
    },
    // GFX10_SW_64K_S_T_RBPLUS_PATINFO[47]: 32 pipes (8 PKRs) 4 bpe @ SW_64K_S_T @ RbPlus
    Pattern { swizzle: Swizzle(17), thick: false, pipes: Some(Pipes::SERIES_X), element_log2: 2, bits: &[
        0, 0, x(0), x(1), y(0), y(1), y(2), x(2), y(3), x(3), y(4) | x(6), x(4) | y(6), x(5) | y(5), x(5), y(6), x(6)]
    },
    // GFX10_SW_64K_S_T_RBPLUS_PATINFO[48]: 32 pipes (8 PKRs) 8 bpe @ SW_64K_S_T @ RbPlus
    Pattern { swizzle: Swizzle(17), thick: false, pipes: Some(Pipes::SERIES_X), element_log2: 3, bits: &[
        0, 0, 0, x(0), y(0), y(1), x(1), x(2), y(2), x(3), y(3) | x(6), x(4) | y(5), y(4) | x(5), x(5), y(5), x(6)]
    },
    // GFX10_SW_64K_S_T_RBPLUS_PATINFO[49]: 32 pipes (8 PKRs) 16 bpe @ SW_64K_S_T @ RbPlus
    Pattern { swizzle: Swizzle(17), thick: false, pipes: Some(Pipes::SERIES_X), element_log2: 4, bits: &[
        0, 0, 0, 0, y(0), y(1), x(0), x(1), y(2), x(2), y(3) | x(5), x(3) | y(5), x(4) | y(4), x(4), y(5), x(5)]
    },
    // GFX10_SW_64K_S_T_RBPLUS_PATINFO[25]: 8 pipes (4 PKRs) 1 bpe @ SW_64K_S_T @ RbPlus
    Pattern { swizzle: Swizzle(17), thick: false, pipes: Some(Pipes::SERIES_S), element_log2: 0, bits: &[
        x(0), x(1), x(2), x(3), y(0), y(1), y(2), y(3), y(4) | x(6), x(4) | y(6), x(5) | y(5), x(5), y(6), x(6), y(7), x(7)]
    },
    // GFX10_SW_64K_S_T_RBPLUS_PATINFO[26]: 8 pipes (4 PKRs) 2 bpe @ SW_64K_S_T @ RbPlus
    Pattern { swizzle: Swizzle(17), thick: false, pipes: Some(Pipes::SERIES_S), element_log2: 1, bits: &[
        0, x(0), x(1), x(2), y(0), y(1), y(2), x(3), y(3) | x(6), x(4) | y(5), y(4) | x(5), x(5), y(5), x(6), y(6), x(7)]
    },
    // GFX10_SW_64K_S_T_RBPLUS_PATINFO[27]: 8 pipes (4 PKRs) 4 bpe @ SW_64K_S_T @ RbPlus
    Pattern { swizzle: Swizzle(17), thick: false, pipes: Some(Pipes::SERIES_S), element_log2: 2, bits: &[
        0, 0, x(0), x(1), y(0), y(1), y(2), x(2), y(3) | x(5), x(3) | y(5), x(4) | y(4), x(4), y(5), x(5), y(6), x(6)]
    },
    // GFX10_SW_64K_S_T_RBPLUS_PATINFO[28]: 8 pipes (4 PKRs) 8 bpe @ SW_64K_S_T @ RbPlus
    Pattern { swizzle: Swizzle(17), thick: false, pipes: Some(Pipes::SERIES_S), element_log2: 3, bits: &[
        0, 0, 0, x(0), y(0), y(1), x(1), x(2), y(2) | x(5), x(3) | y(4), y(3) | x(4), x(4), y(4), x(5), y(5), x(6)]
    },
    // GFX10_SW_64K_S_T_RBPLUS_PATINFO[29]: 8 pipes (4 PKRs) 16 bpe @ SW_64K_S_T @ RbPlus
    Pattern { swizzle: Swizzle(17), thick: false, pipes: Some(Pipes::SERIES_S), element_log2: 4, bits: &[
        0, 0, 0, 0, y(0), y(1), x(0), x(1), y(2) | x(4), x(2) | y(4), x(3) | y(3), x(3), y(4), x(4), y(5), x(5)]
    },
    // GFX10_SW_64K_D_T_RBPLUS_PATINFO[45]: 32 pipes (8 PKRs) 1 bpe @ SW_64K_D_T @ RbPlus
    Pattern { swizzle: Swizzle(18), thick: false, pipes: Some(Pipes::SERIES_X), element_log2: 0, bits: &[
        x(0), x(1), x(2), y(1), y(0), y(2), x(3), y(3), y(4), x(4), y(5) | x(7), x(5) | y(7), x(6) | y(6), x(6), y(7), x(7)]
    },
    // GFX10_SW_64K_D_T_RBPLUS_PATINFO[46]: 32 pipes (8 PKRs) 2 bpe @ SW_64K_D_T @ RbPlus
    Pattern { swizzle: Swizzle(18), thick: false, pipes: Some(Pipes::SERIES_X), element_log2: 1, bits: &[
        0, x(0), x(1), x(2), y(0), y(1), y(2), x(3), y(3), x(4), y(4) | x(7), x(5) | y(6), y(5) | x(6), x(6), y(6), x(7)]
    },
    // GFX10_SW_64K_D_T_RBPLUS_PATINFO[47]: 32 pipes (8 PKRs) 4 bpe @ SW_64K_D_T @ RbPlus
    Pattern { swizzle: Swizzle(18), thick: false, pipes: Some(Pipes::SERIES_X), element_log2: 2, bits: &[
        0, 0, x(0), x(1), y(0), y(1), x(2), y(2), y(3), x(3), y(4) | x(6), x(4) | y(6), x(5) | y(5), x(5), y(6), x(6)]
    },
    // GFX10_SW_64K_D_T_RBPLUS_PATINFO[48]: 32 pipes (8 PKRs) 8 bpe @ SW_64K_D_T @ RbPlus
    Pattern { swizzle: Swizzle(18), thick: false, pipes: Some(Pipes::SERIES_X), element_log2: 3, bits: &[
        0, 0, 0, x(0), y(0), x(1), x(2), y(1), y(2), x(3), y(3) | x(6), x(4) | y(5), y(4) | x(5), x(5), y(5), x(6)]
    },
    // GFX10_SW_64K_D_T_RBPLUS_PATINFO[49]: 32 pipes (8 PKRs) 16 bpe @ SW_64K_D_T @ RbPlus
    Pattern { swizzle: Swizzle(18), thick: false, pipes: Some(Pipes::SERIES_X), element_log2: 4, bits: &[
        0, 0, 0, 0, x(0), y(0), x(1), y(1), y(2), x(2), y(3) | x(5), x(3) | y(5), x(4) | y(4), x(4), y(5), x(5)]
    },
    // GFX10_SW_64K_D_T_RBPLUS_PATINFO[25]: 8 pipes (4 PKRs) 1 bpe @ SW_64K_D_T @ RbPlus
    Pattern { swizzle: Swizzle(18), thick: false, pipes: Some(Pipes::SERIES_S), element_log2: 0, bits: &[
        x(0), x(1), x(2), y(1), y(0), y(2), x(3), y(3), y(4) | x(6), x(4) | y(6), x(5) | y(5), x(5), y(6), x(6), y(7), x(7)]
    },
    // GFX10_SW_64K_D_T_RBPLUS_PATINFO[26]: 8 pipes (4 PKRs) 2 bpe @ SW_64K_D_T @ RbPlus
    Pattern { swizzle: Swizzle(18), thick: false, pipes: Some(Pipes::SERIES_S), element_log2: 1, bits: &[
        0, x(0), x(1), x(2), y(0), y(1), y(2), x(3), y(3) | x(6), x(4) | y(5), y(4) | x(5), x(5), y(5), x(6), y(6), x(7)]
    },
    // GFX10_SW_64K_D_T_RBPLUS_PATINFO[27]: 8 pipes (4 PKRs) 4 bpe @ SW_64K_D_T @ RbPlus
    Pattern { swizzle: Swizzle(18), thick: false, pipes: Some(Pipes::SERIES_S), element_log2: 2, bits: &[
        0, 0, x(0), x(1), y(0), y(1), x(2), y(2), y(3) | x(5), x(3) | y(5), x(4) | y(4), x(4), y(5), x(5), y(6), x(6)]
    },
    // GFX10_SW_64K_D_T_RBPLUS_PATINFO[28]: 8 pipes (4 PKRs) 8 bpe @ SW_64K_D_T @ RbPlus
    Pattern { swizzle: Swizzle(18), thick: false, pipes: Some(Pipes::SERIES_S), element_log2: 3, bits: &[
        0, 0, 0, x(0), y(0), x(1), x(2), y(1), y(2) | x(5), x(3) | y(4), y(3) | x(4), x(4), y(4), x(5), y(5), x(6)]
    },
    // GFX10_SW_64K_D_T_RBPLUS_PATINFO[29]: 8 pipes (4 PKRs) 16 bpe @ SW_64K_D_T @ RbPlus
    Pattern { swizzle: Swizzle(18), thick: false, pipes: Some(Pipes::SERIES_S), element_log2: 4, bits: &[
        0, 0, 0, 0, x(0), y(0), x(1), y(1), y(2) | x(4), x(2) | y(4), x(3) | y(3), x(3), y(4), x(4), y(5), x(5)]
    },
    // GFX10_SW_4K_S_X_RBPLUS_PATINFO[45]: 32 pipes (8 PKRs) 1 bpe @ SW_4K_S_X @ RbPlus
    Pattern { swizzle: Swizzle(21), thick: false, pipes: Some(Pipes::SERIES_X), element_log2: 0, bits: &[
        x(0), x(1), x(2), x(3), y(0), y(1), y(2), y(3), y(4) | x(7), x(4) | y(7), z(2) | y(5) | x(6), z(1) | x(5) | y(6)]
    },
    // GFX10_SW_4K_S_X_RBPLUS_PATINFO[46]: 32 pipes (8 PKRs) 2 bpe @ SW_4K_S_X @ RbPlus
    Pattern { swizzle: Swizzle(21), thick: false, pipes: Some(Pipes::SERIES_X), element_log2: 1, bits: &[
        0, x(0), x(1), x(2), y(0), y(1), y(2), x(3), y(3) | x(7), x(4) | y(6), z(2) | y(4) | x(6), z(1) | x(5) | y(5)]
    },
    // GFX10_SW_4K_S_X_RBPLUS_PATINFO[47]: 32 pipes (8 PKRs) 4 bpe @ SW_4K_S_X @ RbPlus
    Pattern { swizzle: Swizzle(21), thick: false, pipes: Some(Pipes::SERIES_X), element_log2: 2, bits: &[
        0, 0, x(0), x(1), y(0), y(1), y(2), x(2), y(3) | x(6), x(3) | y(6), z(2) | y(4) | x(5), z(1) | x(4) | y(5)]
    },
    // GFX10_SW_4K_S_X_RBPLUS_PATINFO[48]: 32 pipes (8 PKRs) 8 bpe @ SW_4K_S_X @ RbPlus
    Pattern { swizzle: Swizzle(21), thick: false, pipes: Some(Pipes::SERIES_X), element_log2: 3, bits: &[
        0, 0, 0, x(0), y(0), y(1), x(1), x(2), y(2) | x(6), x(3) | y(5), z(2) | y(3) | x(5), z(1) | x(4) | y(4)]
    },
    // GFX10_SW_4K_S_X_RBPLUS_PATINFO[49]: 32 pipes (8 PKRs) 16 bpe @ SW_4K_S_X @ RbPlus
    Pattern { swizzle: Swizzle(21), thick: false, pipes: Some(Pipes::SERIES_X), element_log2: 4, bits: &[
        0, 0, 0, 0, y(0), y(1), x(0), x(1), y(2) | x(5), x(2) | y(5), z(2) | y(3) | x(4), z(1) | x(3) | y(4)]
    },
    // GFX10_SW_4K_S_X_RBPLUS_PATINFO[25]: 8 pipes (4 PKRs) 1 bpe @ SW_4K_S_X @ RbPlus
    Pattern { swizzle: Swizzle(21), thick: false, pipes: Some(Pipes::SERIES_S), element_log2: 0, bits: &[
        x(0), x(1), x(2), x(3), y(0), y(1), y(2), y(3), y(4) | x(6), z(1) | x(4) | y(6), z(0) | x(5) | y(5), x(5)]
    },
    // GFX10_SW_4K_S_X_RBPLUS_PATINFO[26]: 8 pipes (4 PKRs) 2 bpe @ SW_4K_S_X @ RbPlus
    Pattern { swizzle: Swizzle(21), thick: false, pipes: Some(Pipes::SERIES_S), element_log2: 1, bits: &[
        0, x(0), x(1), x(2), y(0), y(1), y(2), x(3), y(3) | x(6), z(1) | x(4) | y(5), z(0) | y(4) | x(5), x(5)]
    },
    // GFX10_SW_4K_S_X_RBPLUS_PATINFO[27]: 8 pipes (4 PKRs) 4 bpe @ SW_4K_S_X @ RbPlus
    Pattern { swizzle: Swizzle(21), thick: false, pipes: Some(Pipes::SERIES_S), element_log2: 2, bits: &[
        0, 0, x(0), x(1), y(0), y(1), y(2), x(2), y(3) | x(5), z(1) | x(3) | y(5), z(0) | x(4) | y(4), x(4)]
    },
    // GFX10_SW_4K_S_X_RBPLUS_PATINFO[28]: 8 pipes (4 PKRs) 8 bpe @ SW_4K_S_X @ RbPlus
    Pattern { swizzle: Swizzle(21), thick: false, pipes: Some(Pipes::SERIES_S), element_log2: 3, bits: &[
        0, 0, 0, x(0), y(0), y(1), x(1), x(2), y(2) | x(5), z(1) | x(3) | y(4), z(0) | y(3) | x(4), x(4)]
    },
    // GFX10_SW_4K_S_X_RBPLUS_PATINFO[29]: 8 pipes (4 PKRs) 16 bpe @ SW_4K_S_X @ RbPlus
    Pattern { swizzle: Swizzle(21), thick: false, pipes: Some(Pipes::SERIES_S), element_log2: 4, bits: &[
        0, 0, 0, 0, y(0), y(1), x(0), x(1), y(2) | x(4), z(1) | x(2) | y(4), z(0) | x(3) | y(3), x(3)]
    },
    // GFX10_SW_4K_D_X_RBPLUS_PATINFO[45]: 32 pipes (8 PKRs) 1 bpe @ SW_4K_D_X @ RbPlus
    Pattern { swizzle: Swizzle(22), thick: false, pipes: Some(Pipes::SERIES_X), element_log2: 0, bits: &[
        x(0), x(1), x(2), y(1), y(0), y(2), x(3), y(3), y(4) | x(7), x(4) | y(7), z(2) | y(5) | x(6), z(1) | x(5) | y(6)]
    },
    // GFX10_SW_4K_D_X_RBPLUS_PATINFO[46]: 32 pipes (8 PKRs) 2 bpe @ SW_4K_D_X @ RbPlus
    Pattern { swizzle: Swizzle(22), thick: false, pipes: Some(Pipes::SERIES_X), element_log2: 1, bits: &[
        0, x(0), x(1), x(2), y(0), y(1), y(2), x(3), y(3) | x(7), x(4) | y(6), z(2) | y(4) | x(6), z(1) | x(5) | y(5)]
    },
    // GFX10_SW_4K_D_X_RBPLUS_PATINFO[47]: 32 pipes (8 PKRs) 4 bpe @ SW_4K_D_X @ RbPlus
    Pattern { swizzle: Swizzle(22), thick: false, pipes: Some(Pipes::SERIES_X), element_log2: 2, bits: &[
        0, 0, x(0), x(1), y(0), y(1), x(2), y(2), y(3) | x(6), x(3) | y(6), z(2) | y(4) | x(5), z(1) | x(4) | y(5)]
    },
    // GFX10_SW_4K_D_X_RBPLUS_PATINFO[48]: 32 pipes (8 PKRs) 8 bpe @ SW_4K_D_X @ RbPlus
    Pattern { swizzle: Swizzle(22), thick: false, pipes: Some(Pipes::SERIES_X), element_log2: 3, bits: &[
        0, 0, 0, x(0), y(0), x(1), x(2), y(1), y(2) | x(6), x(3) | y(5), z(2) | y(3) | x(5), z(1) | x(4) | y(4)]
    },
    // GFX10_SW_4K_D_X_RBPLUS_PATINFO[49]: 32 pipes (8 PKRs) 16 bpe @ SW_4K_D_X @ RbPlus
    Pattern { swizzle: Swizzle(22), thick: false, pipes: Some(Pipes::SERIES_X), element_log2: 4, bits: &[
        0, 0, 0, 0, x(0), y(0), x(1), y(1), y(2) | x(5), x(2) | y(5), z(2) | y(3) | x(4), z(1) | x(3) | y(4)]
    },
    // GFX10_SW_4K_D_X_RBPLUS_PATINFO[25]: 8 pipes (4 PKRs) 1 bpe @ SW_4K_D_X @ RbPlus
    Pattern { swizzle: Swizzle(22), thick: false, pipes: Some(Pipes::SERIES_S), element_log2: 0, bits: &[
        x(0), x(1), x(2), y(1), y(0), y(2), x(3), y(3), y(4) | x(6), z(1) | x(4) | y(6), z(0) | x(5) | y(5), x(5)]
    },
    // GFX10_SW_4K_D_X_RBPLUS_PATINFO[26]: 8 pipes (4 PKRs) 2 bpe @ SW_4K_D_X @ RbPlus
    Pattern { swizzle: Swizzle(22), thick: false, pipes: Some(Pipes::SERIES_S), element_log2: 1, bits: &[
        0, x(0), x(1), x(2), y(0), y(1), y(2), x(3), y(3) | x(6), z(1) | x(4) | y(5), z(0) | y(4) | x(5), x(5)]
    },
    // GFX10_SW_4K_D_X_RBPLUS_PATINFO[27]: 8 pipes (4 PKRs) 4 bpe @ SW_4K_D_X @ RbPlus
    Pattern { swizzle: Swizzle(22), thick: false, pipes: Some(Pipes::SERIES_S), element_log2: 2, bits: &[
        0, 0, x(0), x(1), y(0), y(1), x(2), y(2), y(3) | x(5), z(1) | x(3) | y(5), z(0) | x(4) | y(4), x(4)]
    },
    // GFX10_SW_4K_D_X_RBPLUS_PATINFO[28]: 8 pipes (4 PKRs) 8 bpe @ SW_4K_D_X @ RbPlus
    Pattern { swizzle: Swizzle(22), thick: false, pipes: Some(Pipes::SERIES_S), element_log2: 3, bits: &[
        0, 0, 0, x(0), y(0), x(1), x(2), y(1), y(2) | x(5), z(1) | x(3) | y(4), z(0) | y(3) | x(4), x(4)]
    },
    // GFX10_SW_4K_D_X_RBPLUS_PATINFO[29]: 8 pipes (4 PKRs) 16 bpe @ SW_4K_D_X @ RbPlus
    Pattern { swizzle: Swizzle(22), thick: false, pipes: Some(Pipes::SERIES_S), element_log2: 4, bits: &[
        0, 0, 0, 0, x(0), y(0), x(1), y(1), y(2) | x(4), z(1) | x(2) | y(4), z(0) | x(3) | y(3), x(3)]
    },
    // GFX10_SW_64K_Z_X_1xaa_RBPLUS_PATINFO[45]: 32 pipes (8 PKRs) 1 bpe @ SW_64K_Z_X 1xaa @ RbPlus
    Pattern { swizzle: Swizzle(24), thick: false, pipes: Some(Pipes::SERIES_X), element_log2: 0, bits: &[
        x(0), y(0), x(1), y(1), x(2), y(2), x(3), y(3), y(4) | x(7) | y(7), z(1) | x(4) | y(4), z(0) | y(5) | x(6), x(5) | y(6), x(6) | y(6), y(6), x(7), y(7) | x(8)]
    },
    // GFX10_SW_64K_Z_X_1xaa_RBPLUS_PATINFO[46]: 32 pipes (8 PKRs) 2 bpe @ SW_64K_Z_X 1xaa @ RbPlus
    Pattern { swizzle: Swizzle(24), thick: false, pipes: Some(Pipes::SERIES_X), element_log2: 1, bits: &[
        0, x(0), y(0), x(1), y(1), x(2), y(2), x(3), y(4) | x(7) | y(7), z(1) | x(4) | y(4), z(0) | y(5) | x(6), x(5) | y(6), x(6) | y(6), y(3), y(6), x(7) | y(7)]
    },
    // GFX10_SW_64K_Z_X_1xaa_RBPLUS_PATINFO[47]: 32 pipes (8 PKRs) 4 bpe @ SW_64K_Z_X 1xaa @ RbPlus
    Pattern { swizzle: Swizzle(24), thick: false, pipes: Some(Pipes::SERIES_X), element_log2: 2, bits: &[
        0, 0, x(0), y(0), x(1), y(1), x(2), y(2), y(4) | x(7) | y(7), z(1) | x(4) | y(4), z(0) | y(5) | x(6), x(5) | y(6), x(6) | y(6), x(3), y(3), y(6) | x(7)]
    },
    // GFX10_SW_64K_Z_X_1xaa_RBPLUS_PATINFO[48]: 32 pipes (8 PKRs) 8 bpe @ SW_64K_Z_X 1xaa @ RbPlus
    Pattern { swizzle: Swizzle(24), thick: false, pipes: Some(Pipes::SERIES_X), element_log2: 3, bits: &[
        0, 0, 0, x(0), y(0), x(1), y(1), x(2), y(4) | x(7) | y(7), z(2) | x(4) | y(4), z(1) | y(5) | x(6), z(0) | x(5) | y(6), x(6) | y(6), y(2), x(3), y(3) | x(7)]
    },
    // GFX10_SW_64K_Z_X_1xaa_RBPLUS_PATINFO[49]: 32 pipes (8 PKRs) 16 bpe @ SW_64K_Z_X 1xaa @ RbPlus
    Pattern { swizzle: Swizzle(24), thick: false, pipes: Some(Pipes::SERIES_X), element_log2: 4, bits: &[
        0, 0, 0, 0, x(0), y(0), x(1), y(1), y(4) | x(7) | y(7), z(2) | x(4) | y(4), z(1) | y(5) | x(6), z(0) | x(5) | y(6), x(3) | y(6), x(2), y(2), y(3) | x(6)]
    },
    // GFX10_SW_64K_Z_X_1xaa_RBPLUS_PATINFO[25]: 8 pipes (4 PKRs) 1 bpe @ SW_64K_Z_X 1xaa @ RbPlus
    Pattern { swizzle: Swizzle(24), thick: false, pipes: Some(Pipes::SERIES_S), element_log2: 0, bits: &[
        x(0), y(0), x(1), y(1), x(2), y(2), x(3), y(3), y(4) | x(6) | y(6), z(1) | x(4) | y(4), z(0) | x(5) | y(5), x(5), x(6), y(6) | x(9), x(7) | y(8), y(7) | x(8)]
    },
    // GFX10_SW_64K_Z_X_1xaa_RBPLUS_PATINFO[26]: 8 pipes (4 PKRs) 2 bpe @ SW_64K_Z_X 1xaa @ RbPlus
    Pattern { swizzle: Swizzle(24), thick: false, pipes: Some(Pipes::SERIES_S), element_log2: 1, bits: &[
        0, x(0), y(0), x(1), y(1), x(2), y(2), x(3), y(4) | x(6) | y(6), z(1) | x(4) | y(4), z(0) | x(5) | y(5), y(3), x(5), x(6) | y(8), y(6) | x(8), x(7) | y(7)]
    },
    // GFX10_SW_64K_Z_X_1xaa_RBPLUS_PATINFO[27]: 8 pipes (4 PKRs) 4 bpe @ SW_64K_Z_X 1xaa @ RbPlus
    Pattern { swizzle: Swizzle(24), thick: false, pipes: Some(Pipes::SERIES_S), element_log2: 2, bits: &[
        0, 0, x(0), y(0), x(1), y(1), x(2), y(2), y(4) | x(6) | y(6), z(1) | x(4) | y(4), z(0) | x(5) | y(5), x(3), y(3), x(5) | y(8), x(6) | y(7), y(6) | x(7)]
    },
    // GFX10_SW_64K_Z_X_1xaa_RBPLUS_PATINFO[28]: 8 pipes (4 PKRs) 8 bpe @ SW_64K_Z_X 1xaa @ RbPlus
    Pattern { swizzle: Swizzle(24), thick: false, pipes: Some(Pipes::SERIES_S), element_log2: 3, bits: &[
        0, 0, 0, x(0), y(0), x(1), y(1), x(2), y(4) | x(6) | y(6), z(1) | x(4) | y(4), z(0) | x(5) | y(5), y(2), x(3), y(3) | x(7), x(5) | y(7), x(6) | y(6)]
    },
    // GFX10_SW_64K_Z_X_1xaa_RBPLUS_PATINFO[29]: 8 pipes (4 PKRs) 16 bpe @ SW_64K_Z_X 1xaa @ RbPlus
    Pattern { swizzle: Swizzle(24), thick: false, pipes: Some(Pipes::SERIES_S), element_log2: 4, bits: &[
        0, 0, 0, 0, x(0), y(0), x(1), y(1), y(4) | x(6) | y(6), z(1) | x(4) | y(4), z(0) | x(5) | y(5), x(2), y(2), x(3) | y(7), y(3) | x(6), x(5) | y(6)]
    },
    // GFX10_SW_64K_S_X_RBPLUS_PATINFO[45]: 32 pipes (8 PKRs) 1 bpe @ SW_64K_S_X @ RbPlus
    Pattern { swizzle: Swizzle(25), thick: false, pipes: Some(Pipes::SERIES_X), element_log2: 0, bits: &[
        x(0), x(1), x(2), x(3), y(0), y(1), y(2), y(3), y(4) | x(8), x(4) | y(8), z(2) | y(5) | x(7), z(1) | x(5) | y(7), z(0) | x(6) | y(6), x(6), y(7), x(7)]
    },
    // GFX10_SW_64K_S_X_RBPLUS_PATINFO[46]: 32 pipes (8 PKRs) 2 bpe @ SW_64K_S_X @ RbPlus
    Pattern { swizzle: Swizzle(25), thick: false, pipes: Some(Pipes::SERIES_X), element_log2: 1, bits: &[
        0, x(0), x(1), x(2), y(0), y(1), y(2), x(3), y(3) | x(8), x(4) | y(7), z(2) | y(4) | x(7), z(1) | x(5) | y(6), z(0) | y(5) | x(6), x(6), y(6), x(7)]
    },
    // GFX10_SW_64K_S_X_RBPLUS_PATINFO[47]: 32 pipes (8 PKRs) 4 bpe @ SW_64K_S_X @ RbPlus
    Pattern { swizzle: Swizzle(25), thick: false, pipes: Some(Pipes::SERIES_X), element_log2: 2, bits: &[
        0, 0, x(0), x(1), y(0), y(1), y(2), x(2), y(3) | x(7), x(3) | y(7), z(2) | y(4) | x(6), z(1) | x(4) | y(6), z(0) | x(5) | y(5), x(5), y(6), x(6)]
    },
    // GFX10_SW_64K_S_X_RBPLUS_PATINFO[48]: 32 pipes (8 PKRs) 8 bpe @ SW_64K_S_X @ RbPlus
    Pattern { swizzle: Swizzle(25), thick: false, pipes: Some(Pipes::SERIES_X), element_log2: 3, bits: &[
        0, 0, 0, x(0), y(0), y(1), x(1), x(2), y(2) | x(7), x(3) | y(6), z(2) | y(3) | x(6), z(1) | x(4) | y(5), z(0) | y(4) | x(5), x(5), y(5), x(6)]
    },
    // GFX10_SW_64K_S_X_RBPLUS_PATINFO[49]: 32 pipes (8 PKRs) 16 bpe @ SW_64K_S_X @ RbPlus
    Pattern { swizzle: Swizzle(25), thick: false, pipes: Some(Pipes::SERIES_X), element_log2: 4, bits: &[
        0, 0, 0, 0, y(0), y(1), x(0), x(1), y(2) | x(6), x(2) | y(6), z(2) | y(3) | x(5), z(1) | x(3) | y(5), z(0) | x(4) | y(4), x(4), y(5), x(5)]
    },
    // GFX10_SW_64K_S_X_RBPLUS_PATINFO[25]: 8 pipes (4 PKRs) 1 bpe @ SW_64K_S_X @ RbPlus
    Pattern { swizzle: Swizzle(25), thick: false, pipes: Some(Pipes::SERIES_S), element_log2: 0, bits: &[
        x(0), x(1), x(2), x(3), y(0), y(1), y(2), y(3), y(4) | x(6), z(1) | x(4) | y(6), z(0) | x(5) | y(5), x(5), y(6), x(6), y(7), x(7)]
    },
    // GFX10_SW_64K_S_X_RBPLUS_PATINFO[26]: 8 pipes (4 PKRs) 2 bpe @ SW_64K_S_X @ RbPlus
    Pattern { swizzle: Swizzle(25), thick: false, pipes: Some(Pipes::SERIES_S), element_log2: 1, bits: &[
        0, x(0), x(1), x(2), y(0), y(1), y(2), x(3), y(3) | x(6), z(1) | x(4) | y(5), z(0) | y(4) | x(5), x(5), y(5), x(6), y(6), x(7)]
    },
    // GFX10_SW_64K_S_X_RBPLUS_PATINFO[27]: 8 pipes (4 PKRs) 4 bpe @ SW_64K_S_X @ RbPlus
    Pattern { swizzle: Swizzle(25), thick: false, pipes: Some(Pipes::SERIES_S), element_log2: 2, bits: &[
        0, 0, x(0), x(1), y(0), y(1), y(2), x(2), y(3) | x(5), z(1) | x(3) | y(5), z(0) | x(4) | y(4), x(4), y(5), x(5), y(6), x(6)]
    },
    // GFX10_SW_64K_S_X_RBPLUS_PATINFO[28]: 8 pipes (4 PKRs) 8 bpe @ SW_64K_S_X @ RbPlus
    Pattern { swizzle: Swizzle(25), thick: false, pipes: Some(Pipes::SERIES_S), element_log2: 3, bits: &[
        0, 0, 0, x(0), y(0), y(1), x(1), x(2), y(2) | x(5), z(1) | x(3) | y(4), z(0) | y(3) | x(4), x(4), y(4), x(5), y(5), x(6)]
    },
    // GFX10_SW_64K_S_X_RBPLUS_PATINFO[29]: 8 pipes (4 PKRs) 16 bpe @ SW_64K_S_X @ RbPlus
    Pattern { swizzle: Swizzle(25), thick: false, pipes: Some(Pipes::SERIES_S), element_log2: 4, bits: &[
        0, 0, 0, 0, y(0), y(1), x(0), x(1), y(2) | x(4), z(1) | x(2) | y(4), z(0) | x(3) | y(3), x(3), y(4), x(4), y(5), x(5)]
    },
    // GFX10_SW_64K_D_X_RBPLUS_PATINFO[45]: 32 pipes (8 PKRs) 1 bpe @ SW_64K_D_X @ RbPlus
    Pattern { swizzle: Swizzle(26), thick: false, pipes: Some(Pipes::SERIES_X), element_log2: 0, bits: &[
        x(0), x(1), x(2), y(1), y(0), y(2), x(3), y(3), y(4) | x(8), x(4) | y(8), z(2) | y(5) | x(7), z(1) | x(5) | y(7), z(0) | x(6) | y(6), x(6), y(7), x(7)]
    },
    // GFX10_SW_64K_D_X_RBPLUS_PATINFO[46]: 32 pipes (8 PKRs) 2 bpe @ SW_64K_D_X @ RbPlus
    Pattern { swizzle: Swizzle(26), thick: false, pipes: Some(Pipes::SERIES_X), element_log2: 1, bits: &[
        0, x(0), x(1), x(2), y(0), y(1), y(2), x(3), y(3) | x(8), x(4) | y(7), z(2) | y(4) | x(7), z(1) | x(5) | y(6), z(0) | y(5) | x(6), x(6), y(6), x(7)]
    },
    // GFX10_SW_64K_D_X_RBPLUS_PATINFO[47]: 32 pipes (8 PKRs) 4 bpe @ SW_64K_D_X @ RbPlus
    Pattern { swizzle: Swizzle(26), thick: false, pipes: Some(Pipes::SERIES_X), element_log2: 2, bits: &[
        0, 0, x(0), x(1), y(0), y(1), x(2), y(2), y(3) | x(7), x(3) | y(7), z(2) | y(4) | x(6), z(1) | x(4) | y(6), z(0) | x(5) | y(5), x(5), y(6), x(6)]
    },
    // GFX10_SW_64K_D_X_RBPLUS_PATINFO[48]: 32 pipes (8 PKRs) 8 bpe @ SW_64K_D_X @ RbPlus
    Pattern { swizzle: Swizzle(26), thick: false, pipes: Some(Pipes::SERIES_X), element_log2: 3, bits: &[
        0, 0, 0, x(0), y(0), x(1), x(2), y(1), y(2) | x(7), x(3) | y(6), z(2) | y(3) | x(6), z(1) | x(4) | y(5), z(0) | y(4) | x(5), x(5), y(5), x(6)]
    },
    // GFX10_SW_64K_D_X_RBPLUS_PATINFO[49]: 32 pipes (8 PKRs) 16 bpe @ SW_64K_D_X @ RbPlus
    Pattern { swizzle: Swizzle(26), thick: false, pipes: Some(Pipes::SERIES_X), element_log2: 4, bits: &[
        0, 0, 0, 0, x(0), y(0), x(1), y(1), y(2) | x(6), x(2) | y(6), z(2) | y(3) | x(5), z(1) | x(3) | y(5), z(0) | x(4) | y(4), x(4), y(5), x(5)]
    },
    // GFX10_SW_64K_D_X_RBPLUS_PATINFO[25]: 8 pipes (4 PKRs) 1 bpe @ SW_64K_D_X @ RbPlus
    Pattern { swizzle: Swizzle(26), thick: false, pipes: Some(Pipes::SERIES_S), element_log2: 0, bits: &[
        x(0), x(1), x(2), y(1), y(0), y(2), x(3), y(3), y(4) | x(6), z(1) | x(4) | y(6), z(0) | x(5) | y(5), x(5), y(6), x(6), y(7), x(7)]
    },
    // GFX10_SW_64K_D_X_RBPLUS_PATINFO[26]: 8 pipes (4 PKRs) 2 bpe @ SW_64K_D_X @ RbPlus
    Pattern { swizzle: Swizzle(26), thick: false, pipes: Some(Pipes::SERIES_S), element_log2: 1, bits: &[
        0, x(0), x(1), x(2), y(0), y(1), y(2), x(3), y(3) | x(6), z(1) | x(4) | y(5), z(0) | y(4) | x(5), x(5), y(5), x(6), y(6), x(7)]
    },
    // GFX10_SW_64K_D_X_RBPLUS_PATINFO[27]: 8 pipes (4 PKRs) 4 bpe @ SW_64K_D_X @ RbPlus
    Pattern { swizzle: Swizzle(26), thick: false, pipes: Some(Pipes::SERIES_S), element_log2: 2, bits: &[
        0, 0, x(0), x(1), y(0), y(1), x(2), y(2), y(3) | x(5), z(1) | x(3) | y(5), z(0) | x(4) | y(4), x(4), y(5), x(5), y(6), x(6)]
    },
    // GFX10_SW_64K_D_X_RBPLUS_PATINFO[28]: 8 pipes (4 PKRs) 8 bpe @ SW_64K_D_X @ RbPlus
    Pattern { swizzle: Swizzle(26), thick: false, pipes: Some(Pipes::SERIES_S), element_log2: 3, bits: &[
        0, 0, 0, x(0), y(0), x(1), x(2), y(1), y(2) | x(5), z(1) | x(3) | y(4), z(0) | y(3) | x(4), x(4), y(4), x(5), y(5), x(6)]
    },
    // GFX10_SW_64K_D_X_RBPLUS_PATINFO[29]: 8 pipes (4 PKRs) 16 bpe @ SW_64K_D_X @ RbPlus
    Pattern { swizzle: Swizzle(26), thick: false, pipes: Some(Pipes::SERIES_S), element_log2: 4, bits: &[
        0, 0, 0, 0, x(0), y(0), x(1), y(1), y(2) | x(4), z(1) | x(2) | y(4), z(0) | x(3) | y(3), x(3), y(4), x(4), y(5), x(5)]
    },
    // GFX10_SW_64K_R_X_1xaa_RBPLUS_PATINFO[45]: 32 pipes (8 PKRs) 1 bpe @ SW_64K_R_X 1xaa @ RbPlus
    Pattern { swizzle: Swizzle(27), thick: false, pipes: Some(Pipes::SERIES_X), element_log2: 0, bits: &[
        x(0), x(1), x(2), x(3), y(0), y(1), y(2), y(3), y(4) | x(7) | y(7), z(2) | x(4) | y(4), z(1) | y(5) | x(6), z(0) | x(5) | y(6), x(6) | x(8) | y(8), y(6), x(7), y(7) | x(8)]
    },
    // GFX10_SW_64K_R_X_1xaa_RBPLUS_PATINFO[46]: 32 pipes (8 PKRs) 2 bpe @ SW_64K_R_X 1xaa @ RbPlus
    Pattern { swizzle: Swizzle(27), thick: false, pipes: Some(Pipes::SERIES_X), element_log2: 1, bits: &[
        0, x(0), x(1), x(2), y(0), y(1), y(2), x(3), y(4) | x(7) | y(7), z(2) | x(4) | y(4), z(1) | y(5) | x(6), z(0) | x(5) | y(6), x(6) | x(8) | y(8), y(3), y(6), x(7) | y(7)]
    },
    // GFX10_SW_64K_R_X_1xaa_RBPLUS_PATINFO[47]: 32 pipes (8 PKRs) 4 bpe @ SW_64K_R_X 1xaa @ RbPlus
    Pattern { swizzle: Swizzle(27), thick: false, pipes: Some(Pipes::SERIES_X), element_log2: 2, bits: &[
        0, 0, x(0), x(1), y(0), y(1), x(2), y(2), y(4) | x(7) | y(7), z(2) | x(4) | y(4), z(1) | y(5) | x(6), z(0) | x(5) | y(6), x(6) | x(8) | y(8), x(3), y(3), y(6) | x(7)]
    },
    // GFX10_SW_64K_R_X_1xaa_RBPLUS_PATINFO[48]: 32 pipes (8 PKRs) 8 bpe @ SW_64K_R_X 1xaa @ RbPlus
    Pattern { swizzle: Swizzle(27), thick: false, pipes: Some(Pipes::SERIES_X), element_log2: 3, bits: &[
        0, 0, 0, x(0), y(0), x(1), x(2), y(1), y(4) | x(7) | y(7), z(2) | x(4) | y(4), z(1) | y(5) | x(6), z(0) | x(5) | y(6), x(6) | x(8) | y(8), y(2), x(3), y(3) | x(7)]
    },
    // GFX10_SW_64K_R_X_1xaa_RBPLUS_PATINFO[49]: 32 pipes (8 PKRs) 16 bpe @ SW_64K_R_X 1xaa @ RbPlus
    Pattern { swizzle: Swizzle(27), thick: false, pipes: Some(Pipes::SERIES_X), element_log2: 4, bits: &[
        0, 0, 0, 0, x(0), y(0), x(1), y(1), y(4) | x(7) | y(7), z(2) | x(4) | y(4), z(1) | y(5) | x(6), z(0) | x(5) | y(6), x(3) | x(8) | y(8), x(2), y(2), y(3) | x(6)]
    },
    // GFX10_SW_64K_R_X_1xaa_RBPLUS_PATINFO[25]: 8 pipes (4 PKRs) 1 bpe @ SW_64K_R_X 1xaa @ RbPlus
    Pattern { swizzle: Swizzle(27), thick: false, pipes: Some(Pipes::SERIES_S), element_log2: 0, bits: &[
        x(0), x(1), x(2), x(3), y(0), y(1), y(2), y(3), y(4) | x(6) | y(6), z(1) | x(4) | y(4), z(0) | x(5) | y(5), x(5), x(6), y(6) | x(9), x(7) | y(8), y(7) | x(8)]
    },
    // GFX10_SW_64K_R_X_1xaa_RBPLUS_PATINFO[26]: 8 pipes (4 PKRs) 2 bpe @ SW_64K_R_X 1xaa @ RbPlus
    Pattern { swizzle: Swizzle(27), thick: false, pipes: Some(Pipes::SERIES_S), element_log2: 1, bits: &[
        0, x(0), x(1), x(2), y(0), y(1), y(2), x(3), y(4) | x(6) | y(6), z(1) | x(4) | y(4), z(0) | x(5) | y(5), y(3), x(5), x(6) | y(8), y(6) | x(8), x(7) | y(7)]
    },
    // GFX10_SW_64K_R_X_1xaa_RBPLUS_PATINFO[27]: 8 pipes (4 PKRs) 4 bpe @ SW_64K_R_X 1xaa @ RbPlus
    Pattern { swizzle: Swizzle(27), thick: false, pipes: Some(Pipes::SERIES_S), element_log2: 2, bits: &[
        0, 0, x(0), x(1), y(0), y(1), x(2), y(2), y(4) | x(6) | y(6), z(1) | x(4) | y(4), z(0) | x(5) | y(5), x(3), y(3), x(5) | y(8), x(6) | y(7), y(6) | x(7)]
    },
    // GFX10_SW_64K_R_X_1xaa_RBPLUS_PATINFO[28]: 8 pipes (4 PKRs) 8 bpe @ SW_64K_R_X 1xaa @ RbPlus
    Pattern { swizzle: Swizzle(27), thick: false, pipes: Some(Pipes::SERIES_S), element_log2: 3, bits: &[
        0, 0, 0, x(0), y(0), x(1), x(2), y(1), y(4) | x(6) | y(6), z(1) | x(4) | y(4), z(0) | x(5) | y(5), y(2), x(3), y(3) | x(7), x(5) | y(7), x(6) | y(6)]
    },
    // GFX10_SW_64K_R_X_1xaa_RBPLUS_PATINFO[29]: 8 pipes (4 PKRs) 16 bpe @ SW_64K_R_X 1xaa @ RbPlus
    Pattern { swizzle: Swizzle(27), thick: false, pipes: Some(Pipes::SERIES_S), element_log2: 4, bits: &[
        0, 0, 0, 0, x(0), y(0), x(1), y(1), y(4) | x(6) | y(6), z(1) | x(4) | y(4), z(0) | x(5) | y(5), x(2), y(2), x(3) | y(7), y(3) | x(6), x(5) | y(6)]
    },
    // GFX10_SW_64K_S3_T_RBPLUS_PATINFO[45]: 32 pipes (8 PKRs) 1 bpe @ SW_64K_S3_T @ RbPlus
    Pattern { swizzle: Swizzle(17), thick: true, pipes: Some(Pipes::SERIES_X), element_log2: 0, bits: &[
        x(0), x(1), z(0), y(0), z(1), y(1), x(2), z(2), y(2), x(3), z(3), y(3) | x(5), x(4) | y(4) | z(4), z(4), y(4), x(5)]
    },
    // GFX10_SW_64K_S3_T_RBPLUS_PATINFO[46]: 32 pipes (8 PKRs) 2 bpe @ SW_64K_S3_T @ RbPlus
    Pattern { swizzle: Swizzle(17), thick: true, pipes: Some(Pipes::SERIES_X), element_log2: 1, bits: &[
        0, x(0), z(0), y(0), z(1), y(1), x(1), z(2), y(2), x(2), z(3), y(3) | x(4), x(3) | y(4) | z(4), z(4), y(4), x(4)]
    },
    // GFX10_SW_64K_S3_T_RBPLUS_PATINFO[47]: 32 pipes (8 PKRs) 4 bpe @ SW_64K_S3_T @ RbPlus
    Pattern { swizzle: Swizzle(17), thick: true, pipes: Some(Pipes::SERIES_X), element_log2: 2, bits: &[
        0, 0, x(0), y(0), z(0), y(1), x(1), z(1), y(2), x(2), z(2), y(3) | x(4), x(3) | z(3) | y(4), z(3), y(4), x(4)]
    },
    // GFX10_SW_64K_S3_T_RBPLUS_PATINFO[48]: 32 pipes (8 PKRs) 8 bpe @ SW_64K_S3_T @ RbPlus
    Pattern { swizzle: Swizzle(17), thick: true, pipes: Some(Pipes::SERIES_X), element_log2: 3, bits: &[
        0, 0, 0, x(0), z(0), y(0), x(1), z(1), y(1), x(2), z(2), y(2) | x(4), x(3) | y(3) | z(3), z(3), y(3), x(4)]
    },
    // GFX10_SW_64K_S3_T_RBPLUS_PATINFO[49]: 32 pipes (8 PKRs) 16 bpe @ SW_64K_S3_T @ RbPlus
    Pattern { swizzle: Swizzle(17), thick: true, pipes: Some(Pipes::SERIES_X), element_log2: 4, bits: &[
        0, 0, 0, 0, z(0), y(0), x(0), z(1), y(1), x(1), z(2), y(2) | x(3), x(2) | y(3) | z(3), z(3), y(3), x(3)]
    },
    // GFX10_SW_64K_S3_T_RBPLUS_PATINFO[25]: 8 pipes (4 PKRs) 1 bpe @ SW_64K_S3_T @ RbPlus
    Pattern { swizzle: Swizzle(17), thick: true, pipes: Some(Pipes::SERIES_S), element_log2: 0, bits: &[
        x(0), x(1), z(0), y(0), z(1), y(1), x(2), z(2), y(2) | x(5), x(3) | y(4) | z(4), y(3) | z(3) | x(4), y(3), x(4), z(4), y(4), x(5)]
    },
    // GFX10_SW_64K_S3_T_RBPLUS_PATINFO[26]: 8 pipes (4 PKRs) 2 bpe @ SW_64K_S3_T @ RbPlus
    Pattern { swizzle: Swizzle(17), thick: true, pipes: Some(Pipes::SERIES_S), element_log2: 1, bits: &[
        0, x(0), z(0), y(0), z(1), y(1), x(1), z(2), y(2) | x(4), x(2) | y(4) | z(4), x(3) | y(3) | z(3), y(3), x(3), z(4), y(4), x(4)]
    },
    // GFX10_SW_64K_S3_T_RBPLUS_PATINFO[27]: 8 pipes (4 PKRs) 4 bpe @ SW_64K_S3_T @ RbPlus
    Pattern { swizzle: Swizzle(17), thick: true, pipes: Some(Pipes::SERIES_S), element_log2: 2, bits: &[
        0, 0, x(0), y(0), z(0), y(1), x(1), z(1), y(2) | x(4), x(2) | z(3) | y(4), z(2) | x(3) | y(3), y(3), x(3), z(3), y(4), x(4)]
    },
    // GFX10_SW_64K_S3_T_RBPLUS_PATINFO[28]: 8 pipes (4 PKRs) 8 bpe @ SW_64K_S3_T @ RbPlus
    Pattern { swizzle: Swizzle(17), thick: true, pipes: Some(Pipes::SERIES_S), element_log2: 3, bits: &[
        0, 0, 0, x(0), z(0), y(0), x(1), z(1), y(1) | x(4), x(2) | y(3) | z(3), y(2) | z(2) | x(3), y(2), x(3), z(3), y(3), x(4)]
    },
    // GFX10_SW_64K_S3_T_RBPLUS_PATINFO[29]: 8 pipes (4 PKRs) 16 bpe @ SW_64K_S3_T @ RbPlus
    Pattern { swizzle: Swizzle(17), thick: true, pipes: Some(Pipes::SERIES_S), element_log2: 4, bits: &[
        0, 0, 0, 0, z(0), y(0), x(0), z(1), y(1) | x(3), x(1) | y(3) | z(3), x(2) | y(2) | z(2), y(2), x(2), z(3), y(3), x(3)]
    },
    // GFX10_SW_4K_S3_X_RBPLUS_PATINFO[45]: 32 pipes (8 PKRs) 1 bpe @ SW_4K_S3_X @ RbPlus
    Pattern { swizzle: Swizzle(21), thick: true, pipes: Some(Pipes::SERIES_X), element_log2: 0, bits: &[
        x(0), x(1), z(0), y(0), z(1), y(1), x(2), z(2), y(2) | x(6) | z(6), x(3) | y(5) | z(5), z(3) | y(4) | x(5), y(3) | x(4) | z(4)]
    },
    // GFX10_SW_4K_S3_X_RBPLUS_PATINFO[46]: 32 pipes (8 PKRs) 2 bpe @ SW_4K_S3_X @ RbPlus
    Pattern { swizzle: Swizzle(21), thick: true, pipes: Some(Pipes::SERIES_X), element_log2: 1, bits: &[
        0, x(0), z(0), y(0), z(1), y(1), x(1), z(2), y(2) | x(5) | z(6), x(2) | y(5) | z(5), z(3) | x(4) | y(4), x(3) | y(3) | z(4)]
    },
    // GFX10_SW_4K_S3_X_RBPLUS_PATINFO[47]: 32 pipes (8 PKRs) 4 bpe @ SW_4K_S3_X @ RbPlus
    Pattern { swizzle: Swizzle(21), thick: true, pipes: Some(Pipes::SERIES_X), element_log2: 2, bits: &[
        0, 0, x(0), y(0), z(0), y(1), x(1), z(1), y(2) | x(5) | z(5), x(2) | z(4) | y(5), z(2) | x(4) | y(4), x(3) | y(3) | z(3)]
    },
    // GFX10_SW_4K_S3_X_RBPLUS_PATINFO[48]: 32 pipes (8 PKRs) 8 bpe @ SW_4K_S3_X @ RbPlus
    Pattern { swizzle: Swizzle(21), thick: true, pipes: Some(Pipes::SERIES_X), element_log2: 3, bits: &[
        0, 0, 0, x(0), z(0), y(0), x(1), z(1), y(1) | x(5) | z(5), x(2) | y(4) | z(4), z(2) | y(3) | x(4), y(2) | x(3) | z(3)]
    },
    // GFX10_SW_4K_S3_X_RBPLUS_PATINFO[49]: 32 pipes (8 PKRs) 16 bpe @ SW_4K_S3_X @ RbPlus
    Pattern { swizzle: Swizzle(21), thick: true, pipes: Some(Pipes::SERIES_X), element_log2: 4, bits: &[
        0, 0, 0, 0, z(0), y(0), x(0), z(1), y(1) | x(4) | z(5), x(1) | y(4) | z(4), z(2) | x(3) | y(3), x(2) | y(2) | z(3)]
    },
    // GFX10_SW_4K_S3_X_RBPLUS_PATINFO[25]: 8 pipes (4 PKRs) 1 bpe @ SW_4K_S3_X @ RbPlus
    Pattern { swizzle: Swizzle(21), thick: true, pipes: Some(Pipes::SERIES_S), element_log2: 0, bits: &[
        x(0), x(1), z(0), y(0), z(1), y(1), x(2), z(2), y(2) | x(5) | z(5), x(3) | y(4) | z(4), y(3) | z(3) | x(4), y(3)]
    },
    // GFX10_SW_4K_S3_X_RBPLUS_PATINFO[26]: 8 pipes (4 PKRs) 2 bpe @ SW_4K_S3_X @ RbPlus
    Pattern { swizzle: Swizzle(21), thick: true, pipes: Some(Pipes::SERIES_S), element_log2: 1, bits: &[
        0, x(0), z(0), y(0), z(1), y(1), x(1), z(2), y(2) | x(4) | z(5), x(2) | y(4) | z(4), x(3) | y(3) | z(3), y(3)]
    },
    // GFX10_SW_4K_S3_X_RBPLUS_PATINFO[27]: 8 pipes (4 PKRs) 4 bpe @ SW_4K_S3_X @ RbPlus
    Pattern { swizzle: Swizzle(21), thick: true, pipes: Some(Pipes::SERIES_S), element_log2: 2, bits: &[
        0, 0, x(0), y(0), z(0), y(1), x(1), z(1), y(2) | x(4) | z(4), x(2) | z(3) | y(4), z(2) | x(3) | y(3), y(3)]
    },
    // GFX10_SW_4K_S3_X_RBPLUS_PATINFO[28]: 8 pipes (4 PKRs) 8 bpe @ SW_4K_S3_X @ RbPlus
    Pattern { swizzle: Swizzle(21), thick: true, pipes: Some(Pipes::SERIES_S), element_log2: 3, bits: &[
        0, 0, 0, x(0), z(0), y(0), x(1), z(1), y(1) | x(4) | z(4), x(2) | y(3) | z(3), y(2) | z(2) | x(3), y(2)]
    },
    // GFX10_SW_4K_S3_X_RBPLUS_PATINFO[29]: 8 pipes (4 PKRs) 16 bpe @ SW_4K_S3_X @ RbPlus
    Pattern { swizzle: Swizzle(21), thick: true, pipes: Some(Pipes::SERIES_S), element_log2: 4, bits: &[
        0, 0, 0, 0, z(0), y(0), x(0), z(1), y(1) | x(3) | z(4), x(1) | y(3) | z(3), x(2) | y(2) | z(2), y(2)]
    },
    // GFX10_SW_64K_S3_X_RBPLUS_PATINFO[45]: 32 pipes (8 PKRs) 1 bpe @ SW_64K_S3_X @ RbPlus
    Pattern { swizzle: Swizzle(25), thick: true, pipes: Some(Pipes::SERIES_X), element_log2: 0, bits: &[
        x(0), x(1), z(0), y(0), z(1), y(1), x(2), z(2), y(2) | x(7) | z(7), x(3) | y(6) | z(6), z(3) | y(5) | x(6), y(3) | x(5) | z(5), x(4) | y(4) | z(4), z(4), y(4), x(5)]
    },
    // GFX10_SW_64K_S3_X_RBPLUS_PATINFO[46]: 32 pipes (8 PKRs) 2 bpe @ SW_64K_S3_X @ RbPlus
    Pattern { swizzle: Swizzle(25), thick: true, pipes: Some(Pipes::SERIES_X), element_log2: 1, bits: &[
        0, x(0), z(0), y(0), z(1), y(1), x(1), z(2), y(2) | x(6) | z(7), x(2) | y(6) | z(6), z(3) | x(5) | y(5), y(3) | x(4) | z(5), x(3) | y(4) | z(4), z(4), y(4), x(4)]
    },
    // GFX10_SW_64K_S3_X_RBPLUS_PATINFO[47]: 32 pipes (8 PKRs) 4 bpe @ SW_64K_S3_X @ RbPlus
    Pattern { swizzle: Swizzle(25), thick: true, pipes: Some(Pipes::SERIES_X), element_log2: 2, bits: &[
        0, 0, x(0), y(0), z(0), y(1), x(1), z(1), y(2) | x(6) | z(6), x(2) | z(5) | y(6), z(2) | x(5) | y(5), y(3) | x(4) | z(4), x(3) | z(3) | y(4), z(3), y(4), x(4)]
    },
    // GFX10_SW_64K_S3_X_RBPLUS_PATINFO[48]: 32 pipes (8 PKRs) 8 bpe @ SW_64K_S3_X @ RbPlus
    Pattern { swizzle: Swizzle(25), thick: true, pipes: Some(Pipes::SERIES_X), element_log2: 3, bits: &[
        0, 0, 0, x(0), z(0), y(0), x(1), z(1), y(1) | x(6) | z(6), x(2) | y(5) | z(5), z(2) | y(4) | x(5), y(2) | x(4) | z(4), x(3) | y(3) | z(3), z(3), y(3), x(4)]
    },
    // GFX10_SW_64K_S3_X_RBPLUS_PATINFO[49]: 32 pipes (8 PKRs) 16 bpe @ SW_64K_S3_X @ RbPlus
    Pattern { swizzle: Swizzle(25), thick: true, pipes: Some(Pipes::SERIES_X), element_log2: 4, bits: &[
        0, 0, 0, 0, z(0), y(0), x(0), z(1), y(1) | x(5) | z(6), x(1) | y(5) | z(5), z(2) | x(4) | y(4), y(2) | x(3) | z(4), x(2) | y(3) | z(3), z(3), y(3), x(3)]
    },
    // GFX10_SW_64K_S3_X_RBPLUS_PATINFO[25]: 8 pipes (4 PKRs) 1 bpe @ SW_64K_S3_X @ RbPlus
    Pattern { swizzle: Swizzle(25), thick: true, pipes: Some(Pipes::SERIES_S), element_log2: 0, bits: &[
        x(0), x(1), z(0), y(0), z(1), y(1), x(2), z(2), y(2) | x(5) | z(5), x(3) | y(4) | z(4), y(3) | z(3) | x(4), y(3), x(4), z(4), y(4), x(5)]
    },
    // GFX10_SW_64K_S3_X_RBPLUS_PATINFO[26]: 8 pipes (4 PKRs) 2 bpe @ SW_64K_S3_X @ RbPlus
    Pattern { swizzle: Swizzle(25), thick: true, pipes: Some(Pipes::SERIES_S), element_log2: 1, bits: &[
        0, x(0), z(0), y(0), z(1), y(1), x(1), z(2), y(2) | x(4) | z(5), x(2) | y(4) | z(4), x(3) | y(3) | z(3), y(3), x(3), z(4), y(4), x(4)]
    },
    // GFX10_SW_64K_S3_X_RBPLUS_PATINFO[27]: 8 pipes (4 PKRs) 4 bpe @ SW_64K_S3_X @ RbPlus
    Pattern { swizzle: Swizzle(25), thick: true, pipes: Some(Pipes::SERIES_S), element_log2: 2, bits: &[
        0, 0, x(0), y(0), z(0), y(1), x(1), z(1), y(2) | x(4) | z(4), x(2) | z(3) | y(4), z(2) | x(3) | y(3), y(3), x(3), z(3), y(4), x(4)]
    },
    // GFX10_SW_64K_S3_X_RBPLUS_PATINFO[28]: 8 pipes (4 PKRs) 8 bpe @ SW_64K_S3_X @ RbPlus
    Pattern { swizzle: Swizzle(25), thick: true, pipes: Some(Pipes::SERIES_S), element_log2: 3, bits: &[
        0, 0, 0, x(0), z(0), y(0), x(1), z(1), y(1) | x(4) | z(4), x(2) | y(3) | z(3), y(2) | z(2) | x(3), y(2), x(3), z(3), y(3), x(4)]
    },
    // GFX10_SW_64K_S3_X_RBPLUS_PATINFO[29]: 8 pipes (4 PKRs) 16 bpe @ SW_64K_S3_X @ RbPlus
    Pattern { swizzle: Swizzle(25), thick: true, pipes: Some(Pipes::SERIES_S), element_log2: 4, bits: &[
        0, 0, 0, 0, z(0), y(0), x(0), z(1), y(1) | x(3) | z(4), x(1) | y(3) | z(3), x(2) | y(2) | z(2), y(2), x(2), z(3), y(3), x(3)]
    },
    // GFX10_SW_64K_D3_X_RBPLUS_PATINFO[45]: 32 pipes (8 PKRs) 1 bpe @ SW_64K_D3_X @ RbPlus
    Pattern { swizzle: Swizzle(26), thick: true, pipes: Some(Pipes::SERIES_X), element_log2: 0, bits: &[
        x(0), x(1), z(0), y(0), y(1), z(1), x(2), z(2), y(4) | x(7) | y(7), x(4) | y(4), y(2) | y(5) | x(6), x(5) | y(6), z(3) | x(7), y(3), x(3), z(4)]
    },
    // GFX10_SW_64K_D3_X_RBPLUS_PATINFO[46]: 32 pipes (8 PKRs) 2 bpe @ SW_64K_D3_X @ RbPlus
    Pattern { swizzle: Swizzle(26), thick: true, pipes: Some(Pipes::SERIES_X), element_log2: 1, bits: &[
        0, x(0), z(0), y(0), x(1), z(1), y(1), z(2), y(4) | x(7) | y(7), x(4) | y(4), y(2) | y(5) | x(6), y(3) | x(5) | y(6), z(3) | x(7), x(2), z(4), x(3)]
    },
    // GFX10_SW_64K_D3_X_RBPLUS_PATINFO[47]: 32 pipes (8 PKRs) 4 bpe @ SW_64K_D3_X @ RbPlus
    Pattern { swizzle: Swizzle(26), thick: true, pipes: Some(Pipes::SERIES_X), element_log2: 2, bits: &[
        0, 0, x(0), y(0), x(1), z(0), y(1), z(1), y(4) | x(7) | y(7), x(4) | y(4), y(2) | y(5) | x(6), y(3) | x(5) | y(6), z(2) | x(7), x(2), z(3), x(3)]
    },
    // GFX10_SW_64K_D3_X_RBPLUS_PATINFO[48]: 32 pipes (8 PKRs) 8 bpe @ SW_64K_D3_X @ RbPlus
    Pattern { swizzle: Swizzle(26), thick: true, pipes: Some(Pipes::SERIES_X), element_log2: 3, bits: &[
        0, 0, 0, x(0), y(0), z(0), x(1), z(1), z(3) | y(4) | x(7) | y(7), x(4) | y(4), y(1) | y(5) | x(6), y(2) | x(5) | y(6), z(2) | x(7), x(2), y(3), x(3)]
    },
    // GFX10_SW_64K_D3_X_RBPLUS_PATINFO[49]: 32 pipes (8 PKRs) 16 bpe @ SW_64K_D3_X @ RbPlus
    Pattern { swizzle: Swizzle(26), thick: true, pipes: Some(Pipes::SERIES_X), element_log2: 4, bits: &[
        0, 0, 0, 0, x(0), z(0), y(0), z(1), z(3) | y(4) | x(7) | y(7), y(1) | x(4) | y(4), x(1) | y(5) | x(6), y(2) | x(5) | y(6), z(2) | x(7), x(2), y(3), x(3)]
    },
    // GFX10_SW_64K_D3_X_RBPLUS_PATINFO[25]: 8 pipes (4 PKRs) 1 bpe @ SW_64K_D3_X @ RbPlus
    Pattern { swizzle: Swizzle(26), thick: true, pipes: Some(Pipes::SERIES_S), element_log2: 0, bits: &[
        x(0), x(1), z(0), y(0), y(1), z(1), x(2), z(2), y(4) | x(6) | y(6), x(4) | y(4), x(5) | y(5), y(2), y(3), z(3), x(3), z(4)]
    },
    // GFX10_SW_64K_D3_X_RBPLUS_PATINFO[26]: 8 pipes (4 PKRs) 2 bpe @ SW_64K_D3_X @ RbPlus
    Pattern { swizzle: Swizzle(26), thick: true, pipes: Some(Pipes::SERIES_S), element_log2: 1, bits: &[
        0, x(0), z(0), y(0), x(1), z(1), y(1), z(2), y(4) | x(6) | y(6), x(4) | y(4), y(2) | x(5) | y(5), z(3), y(3), x(2), z(4), x(3)]
    },
    // GFX10_SW_64K_D3_X_RBPLUS_PATINFO[27]: 8 pipes (4 PKRs) 4 bpe @ SW_64K_D3_X @ RbPlus
    Pattern { swizzle: Swizzle(26), thick: true, pipes: Some(Pipes::SERIES_S), element_log2: 2, bits: &[
        0, 0, x(0), y(0), x(1), z(0), y(1), z(1), y(4) | x(6) | y(6), x(4) | y(4), y(2) | x(5) | y(5), z(2), y(3), x(2), z(3), x(3)]
    },
    // GFX10_SW_64K_D3_X_RBPLUS_PATINFO[28]: 8 pipes (4 PKRs) 8 bpe @ SW_64K_D3_X @ RbPlus
    Pattern { swizzle: Swizzle(26), thick: true, pipes: Some(Pipes::SERIES_S), element_log2: 3, bits: &[
        0, 0, 0, x(0), y(0), z(0), x(1), z(1), z(3) | y(4) | x(6) | y(6), x(4) | y(4), y(1) | x(5) | y(5), z(2), y(2), x(2), y(3), x(3)]
    },
    // GFX10_SW_64K_D3_X_RBPLUS_PATINFO[29]: 8 pipes (4 PKRs) 16 bpe @ SW_64K_D3_X @ RbPlus
    Pattern { swizzle: Swizzle(26), thick: true, pipes: Some(Pipes::SERIES_S), element_log2: 4, bits: &[
        0, 0, 0, 0, x(0), z(0), y(0), z(1), z(3) | y(4) | x(6) | y(6), y(1) | x(4) | y(4), x(1) | x(5) | y(5), z(2), y(2), x(2), y(3), x(3)]
    },
];
