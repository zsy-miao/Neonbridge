// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Shims.D3D12X.Binding;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>Which tables a draw mirrors further for its programs' reach (--mirror-table-records reach).</summary>
public sealed class TableReachesTests
{
    [Fact]
    public void Only_tables_read_past_what_binds_mirror_are_mirrored_further_in_whole_records()
    {
        // TL_PS_PBR_MATERIAL_STANDARD: table 0 in s8 read to byte 0x1690 (record 180), table 1 in s9 to 0x190.
        var reach = new Dictionary<int, uint> { [8] = 0x1690, [9] = 0x190, [10] = 0x801 };

        Assert.Equal([new TableReachEntry(8, 181), new TableReachEntry(10, 65)], TableReaches.Entries(reach, DescriptorWindow.DefaultTableRecords));
        Assert.Empty(TableReaches.Entries(reach, DescriptorWindow.MaxTableRecords));
    }

    [Fact]
    public void A_translated_program_reads_the_reach_of_the_extracted_program_it_comes_from()
    {
        Assert.Equal("sgd1_0b12c1ad_t32_TL_PS_PBR_MATERIAL_STANDARD_42ad1e50_Wrapped", TableReaches.Source("sgd1_0b12c1ad_t32_TL_PS_PBR_MATERIAL_STANDARD_42ad1e50_Wrapped"));
        Assert.Equal("sgd1_04650ba8_t32_VS_TiledLighting_fdbb4ddc_Wrapped", TableReaches.Source("sgd1_04650ba8_t32_VS_TiledLighting_fdbb4ddc_Wrapped.link-beb72bb57485"));
        Assert.Equal("sgd1_0ae8d77c_t32_PS_SSSResolve_44ece319_Wrapped", TableReaches.Source("sgd1_0ae8d77c_t32_PS_SSSResolve_44ece319_Wrapped.trim2"));
    }
}
