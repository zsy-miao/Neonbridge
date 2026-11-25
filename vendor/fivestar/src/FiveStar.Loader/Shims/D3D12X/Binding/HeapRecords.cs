// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Binding;

/// <summary>Where a heap's records lie in the <see cref="DescriptorWindow"/> and which desktop indices a record may name.</summary>
/// <param name="Base">Window offset of record 0.</param>
/// <param name="RecordBytes">Window bytes per record (a T# or an S#).</param>
/// <param name="FirstDesktop">The lowest desktop index a game descriptor takes.</param>
/// <param name="DesktopCount">The desktop heap's size.</param>
/// <param name="NullIndex">The desktop index a record that names nothing reads.</param>
internal readonly record struct HeapRecords(uint Base, uint RecordBytes, uint FirstDesktop, uint DesktopCount, uint NullIndex);
