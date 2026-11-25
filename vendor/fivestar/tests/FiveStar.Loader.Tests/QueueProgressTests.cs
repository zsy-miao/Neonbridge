// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Shims.D3D12X;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>The removal report's GPU memory line: the segment groups and what is open in place, or why they are unread.</summary>
public sealed class QueueProgressTests
{
    [Fact]
    public void The_removal_report_carries_the_gpu_memory_line_or_why_it_is_missing()
    {
        try
        {
            QueueProgress.Memory = () => "GPU memory: local 9.80 of 11.20 GiB budget; system 6.10 of 7.80 GiB budget";
            string read = QueueProgress.MemoryLine();
            QueueProgress.Memory = () => throw new InvalidOperationException("adapter gone");
            string failed = QueueProgress.MemoryLine();

            Assert.Equal("; GPU memory: local 9.80 of 11.20 GiB budget; system 6.10 of 7.80 GiB budget", read);
            Assert.Equal("; GPU memory unread: adapter gone", failed);
        }
        finally
        {
            QueueProgress.Memory = null;
        }
    }
}
