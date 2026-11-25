// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Formats.Rpf;

namespace FiveStar.Formats.Models;

/// <summary>
/// Pairs carved resource blobs with the physical segment that follows each (<see cref="BlobKind.Phys"/>) and reads
/// their drawables. When a virtual segment is a whole number of 64 KiB chunks the carver joins the physical run to it;
/// such a blob is split at the first 64 KiB boundary where every geometry decodes.
/// </summary>
public static class DrawableScanner
{
    public static IEnumerable<CarvedDrawable> Scan(IEnumerable<CarvedBlob> blobs)
    {
        CarvedBlob? pending = null;

        foreach (var blob in blobs)
        {
            if (pending is not null)
            {
                var physical = Array.Empty<byte>();

                if (blob.Kind == BlobKind.Phys)
                {
                    physical = blob.Data;
                }

                foreach (var drawable in Read(pending, physical))
                {
                    yield return drawable;
                }

                pending = null;
            }

            if (blob.Kind == BlobKind.Rsc)
            {
                pending = blob;
            }
        }

        if (pending is not null)
        {
            foreach (var drawable in Read(pending, Array.Empty<byte>()))
            {
                yield return drawable;
            }
        }
    }

    private static IReadOnlyList<CarvedDrawable> Read(CarvedBlob resource, byte[] physical)
    {
        if (physical.Length > 0)
        {
            return Read(resource, resource.Data, physical);
        }

        var joined = resource.Data.AsMemory();

        for (var split = ZlibStreamProbe.ChunkSize; split < joined.Length; split += ZlibStreamProbe.ChunkSize)
        {
            var drawables = Read(resource, joined[..split], joined[split..]);

            if (drawables.Count > 0 && drawables.All(carved => carved.Drawable.Problems.Count == 0))
            {
                return drawables;
            }
        }

        return Read(resource, resource.Data, Array.Empty<byte>());
    }

    private static List<CarvedDrawable> Read(CarvedBlob resource, ReadOnlyMemory<byte> virtualSegment, ReadOnlyMemory<byte> physicalSegment)
    {
        var view = new ResourceView(virtualSegment, physicalSegment);
        var hasPhysical = physicalSegment.Length > 0;

        return DrawableReader.Read(view).Select(drawable => new CarvedDrawable(resource.Offset, hasPhysical, drawable)).ToList();
    }
}
