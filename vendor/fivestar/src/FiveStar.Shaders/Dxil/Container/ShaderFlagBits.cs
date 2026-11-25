// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil.Container;

/// <summary>DXIL shader flags (entry property 0); the validator recomputes them from the module and compares.</summary>
[Flags]
public enum ShaderFlagBits : ulong
{
    None = 0,
    EnableDoublePrecision = 1UL << 2,
    EnableRawAndStructuredBuffers = 1UL << 4,
    EnableDoubleExtensions = 1UL << 6,
    StencilExport = 1UL << 11,
    TypedUavLoadAdditionalFormats = 1UL << 13,
    UavsAtEveryStage = 1UL << 16,
    WaveOps = 1UL << 19,
    Int64Ops = 1UL << 20,
    Barycentrics = 1UL << 22,
    AtomicInt64OnTypedResource = 1UL << 27,
    AtomicInt64OnGroupShared = 1UL << 28,
    ResourceDescriptorHeapIndexing = 1UL << 30,
    SamplerDescriptorHeapIndexing = 1UL << 31,
    AtomicInt64OnHeapResource = 1UL << 32,

    /// <summary>Shader model 6.7 texture operations: offsets that are not immediates, SampleCmpLevel.</summary>
    AdvancedTextureOps = 1UL << 34,
}
