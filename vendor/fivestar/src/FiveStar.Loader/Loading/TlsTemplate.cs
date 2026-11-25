// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Loading;

/// <summary>
/// The TLS directory of a mapped image as the TLS host needs it: the relocated template bytes, zero-fill size,
/// characteristics, callback addresses, the image base, the address of the image's own _tls_index, and whether the\n/// image is an executable, whose code reads TLS slot 0 (<see cref="TlsSlotZero"/>).
/// </summary>
internal sealed record TlsTemplate(byte[] RawData, uint ZeroFill, uint Characteristics, IReadOnlyList<ulong> Callbacks, ulong ImageBase, ulong IndexAddress, bool OwnsSlotZero);
