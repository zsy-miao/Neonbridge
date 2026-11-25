// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.D3D12X.Resources;

/// <summary>
/// The desktop textures standing for console GPU address ranges, sorted by address. Lookups read the current array
/// without a lock; registering replaces it. A new texture retires every texture whose range it overlaps (the memory
/// was given to something else). The newest <see cref="KeptAliases"/> retired textures over exactly one range (other
/// views of the same memory) keep their reference until disposal, as descriptors the game wrote earlier may still name
/// them; older ones are released <see cref="AliasGraceFrames"/> presents later. Any other retired texture is
/// released <see cref="GraceFrames"/> presents later. Either is released only once every queue has completed the work
/// it had submitted when the texture was retired (its shim fences, <see cref="QueueProgress"/>), so no list that could
/// sample it is still on the GPU; the presents give the game time to rewrite its descriptors for the new contents.
/// The grace was 120 and 600 presents, without the fences: boot w36 held 4,905 retired textures, their video memory
/// charged to commit, when commit reached its limit. Before that every streamed-out texture stayed alive: a cutscene's
/// streaming grew the process by about 3 GB a minute once video memory spilled into system memory.
/// </summary>
internal sealed class GpuTextureRegistry : IDisposable
{
    /// <summary>Presents between retiring a texture whose memory was reused and releasing it, its submitted work done.</summary>
    public const long GraceFrames = 8;

    /// <summary>
    /// Retired textures over exactly one range kept until disposal, the newest ones, by default (<c>--kept-aliases</c>). Two
    /// kept a second texture alive per reused streaming slot, its video memory charged to commit through the driver's
    /// write-combined mirror; one keeps the latest the game's older descriptors may still name, and a placed texture's heap range stays mapped
    /// until its texture goes, so a stale read never reaches freed memory (issue #23).
    /// </summary>
    public const int DefaultKeptAliases = 1;

    /// <summary>Retired textures over exactly one range kept until disposal for registries made from now on (<c>--kept-aliases</c>).</summary>
    public static int KeptAliases { get; set; } = DefaultKeptAliases;

    /// <summary>Presents before an older texture over the same range is released, its submitted work done.</summary>
    public const long AliasGraceFrames = 30;

    private readonly object _gate = new();
    private readonly Dictionary<(ulong Address, ulong Size), Queue<GpuTexture>> _aliases = [];
    private readonly Queue<(GpuTexture Texture, long Frame, ulong[] Submitted)> _retired = new();
    private readonly Queue<(GpuTexture Texture, long Frame, ulong[] Submitted)> _agedAliases = new();
    private readonly Func<ulong[]> _submitted;
    private readonly Func<ulong[]> _completed;
    private int _keptAliases = KeptAliases;
    private int _aliasCount;
    private GpuTexture[] _sorted = [];
    private long _frame;
    private long _released;

    /// <summary>The registry of the loader's queues (<see cref="QueueProgress"/>).</summary>
    public static GpuTextureRegistry OfQueues()
    {
        return OfQueues(KeptAliases);
    }

    /// <summary>A registry over the shim queues' fences keeping <paramref name="kept"/> retired textures per range.</summary>
    public static GpuTextureRegistry OfQueues(int kept)
    {
        return new GpuTextureRegistry(QueueProgress.Submitted, QueueProgress.Completed) { _keptAliases = kept };
    }

    /// <param name="submitted">Every queue's submitted fence value now.</param>
    /// <param name="completed">Every queue's completed fence value now.</param>
    public GpuTextureRegistry(Func<ulong[]> submitted, Func<ulong[]> completed)
    {
        _submitted = submitted;
        _completed = completed;
    }

    /// <summary>Whether every queue has completed what it had <paramref name="submitted"/>.</summary>
    public static bool Finished(ulong[] submitted, ulong[] completed)
    {
        for (int queue = 0; queue < submitted.Length; queue++)
        {
            if (queue >= completed.Length || completed[queue] < submitted[queue])
            {
                return false;
            }
        }

        return true;
    }

    public int Count => Volatile.Read(ref _sorted).Length;

    /// <summary>Retired textures over one range this registry keeps until disposal.</summary>
    public int Kept => _keptAliases;

    /// <summary>Retired textures still holding their reference.</summary>
    public int RetiredCount
    {
        get
        {
            lock (_gate)
            {
                return _aliasCount + _retired.Count + _agedAliases.Count;
            }
        }
    }

    /// <summary>Retired textures released so far.</summary>
    public long Released => Interlocked.Read(ref _released);

    /// <summary>"N textures, retired R (aliased A), released N", for the texture counters.</summary>
    public string Summary
    {
        get
        {
            lock (_gate)
            {
                int retired = _aliasCount + _retired.Count + _agedAliases.Count;

                return $"{_sorted.Length} textures, retired {retired} (aliased {_aliasCount + _agedAliases.Count}), released {_released}";
            }
        }
    }

    public void Register(GpuTexture texture)
    {
        lock (_gate)
        {
            var kept = new List<GpuTexture>(_sorted.Length + 1);

            foreach (var entry in _sorted)
            {
                if (entry.Address < texture.Address + texture.Size && texture.Address < entry.Address + entry.Size)
                {
                    Retire(entry, texture);
                }
                else
                {
                    kept.Add(entry);
                }
            }

            kept.Add(texture);
            kept.Sort((left, right) => left.Address.CompareTo(right.Address));
            Volatile.Write(ref _sorted, kept.ToArray());
        }
    }

    /// <summary>Once per present: releases the retired textures whose grace period ended.</summary>
    public void Advance(long frame)
    {
        lock (_gate)
        {
            _frame = frame;

            if (_retired.Count == 0 && _agedAliases.Count == 0)
            {
                return;
            }

            var completed = _completed();
            ReleaseDue(_retired, frame - GraceFrames, completed);
            ReleaseDue(_agedAliases, frame - AliasGraceFrames, completed);
        }
    }

    /// <summary>Releases, oldest first, the textures retired by <paramref name="lastFrame"/> whose submitted work is done.</summary>
    private void ReleaseDue(Queue<(GpuTexture Texture, long Frame, ulong[] Submitted)> queue, long lastFrame, ulong[] completed)
    {
        while (queue.Count > 0 && queue.Peek().Frame <= lastFrame && Finished(queue.Peek().Submitted, completed))
        {
            ComObject.Release(queue.Dequeue().Texture.Desktop);
            _released++;
        }
    }

    /// <summary>The texture whose range holds <paramref name="address"/>, or null.</summary>
    [HotPath]
    public GpuTexture? Find(ulong address)
    {
        var sorted = Volatile.Read(ref _sorted);
        int low = 0;
        int high = sorted.Length - 1;

        while (low <= high)
        {
            int middle = low + ((high - low) >> 1);
            var entry = sorted[middle];

            if (entry.Contains(address))
            {
                return entry;
            }

            if (address < entry.Address)
            {
                high = middle - 1;
            }
            else
            {
                low = middle + 1;
            }
        }

        return null;
    }

    public void Dispose()
    {
        lock (_gate)
        {
            var aliases = _aliases.Values.SelectMany(queue => queue);
            var retired = _retired.Concat(_agedAliases).Select(entry => entry.Texture);

            foreach (var entry in _sorted.Concat(aliases).Concat(retired))
            {
                ComObject.Release(entry.Desktop);
            }

            _aliases.Clear();
            _aliasCount = 0;
            _retired.Clear();
            _agedAliases.Clear();
            _sorted = [];
        }
    }

    private void Retire(GpuTexture entry, GpuTexture replacement)
    {
        if (entry.Address == replacement.Address && entry.Size == replacement.Size)
        {
            Alias(entry);

            return;
        }

        _retired.Enqueue((entry, _frame, _submitted()));
    }

    /// <summary>
    /// Keeps a texture retired by another over exactly its range; past <see cref="KeptAliases"/> for that range the
    /// oldest is released <see cref="AliasGraceFrames"/> presents later. All were kept before, and a streaming pool
    /// reuses fixed-size slots: the growth after the story load followed the textures made.
    /// </summary>
    private void Alias(GpuTexture entry)
    {
        var range = (entry.Address, entry.Size);

        if (!_aliases.TryGetValue(range, out var kept))
        {
            kept = new Queue<GpuTexture>();
            _aliases[range] = kept;
        }

        kept.Enqueue(entry);
        _aliasCount++;

        if (kept.Count > _keptAliases)
        {
            _agedAliases.Enqueue((kept.Dequeue(), _frame, _submitted()));
            _aliasCount--;
        }
    }
}
