// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Binding;

/// <summary>The descriptor a buffer record was written for (<see cref="BufferRecords.Based"/>).</summary>
internal enum ViewKind
{
    ShaderResource,
    UnorderedAccess,
    ConstantBuffer,
    AddressView,
}
