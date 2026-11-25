// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil.Ir;

/// <summary>The LLVM instructions DXIL allows that the backend emits.</summary>
public enum Opcode
{
    Binary,
    Cast,
    Compare,
    Select,
    ExtractValue,
    Call,
    Phi,
    GetElementPtr,
    Load,
    Store,
    AtomicRmw,
    CompareExchange,
    Alloca,
    Branch,
    ConditionalBranch,
    Switch,
    Return,
}
