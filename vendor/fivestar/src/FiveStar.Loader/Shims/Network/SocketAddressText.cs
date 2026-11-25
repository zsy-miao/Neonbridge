// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using System.Globalization;
using System.Net;
using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.Network;

/// <summary>A SOCKADDR (ws2def.h) as "address:port", for the log of blocked attempts.</summary>
internal static unsafe class SocketAddressText
{
    /// <summary>AF_INET: sockaddr_in { u16 family; u16 port (big endian); u8 addr[4]; ... }.</summary>
    private const ushort FamilyInet = 2;

    /// <summary>AF_INET6: sockaddr_in6 { u16 family; u16 port; u32 flowinfo; u8 addr[16]; u32 scope; }.</summary>
    private const ushort FamilyInet6 = 23;

    private const int PortOffset = 2;
    private const int Inet4AddressOffset = 4;
    private const int Inet4AddressSize = 4;
    private const int Inet6AddressOffset = 8;
    private const int Inet6AddressSize = 16;

    public static string Format(byte* address, int length)
    {
        if (address == null || length < Inet4AddressOffset + Inet4AddressSize)
        {
            return "(no address)";
        }

        ushort family = *(ushort*)address;
        ushort port = BinaryPrimitives.ReverseEndianness(*(ushort*)(address + PortOffset));
        string portText = port.ToString(CultureInfo.InvariantCulture);

        if (family == FamilyInet)
        {
            return $"{new IPAddress(new ReadOnlySpan<byte>(address + Inet4AddressOffset, Inet4AddressSize))}:{portText}";
        }

        if (family == FamilyInet6 && length >= Inet6AddressOffset + Inet6AddressSize)
        {
            return $"[{new IPAddress(new ReadOnlySpan<byte>(address + Inet6AddressOffset, Inet6AddressSize))}]:{portText}";
        }

        return $"(family {family.ToString(CultureInfo.InvariantCulture)})";
    }

    /// <summary>"host:service" of a name lookup, either part possibly null.</summary>
    public static string Lookup(string? host, string? service)
    {
        return $"{host ?? "(null)"}:{service ?? "(null)"}";
    }

    public static string? Ansi(byte* text)
    {
        if (text == null)
        {
            return null;
        }

        return Marshal.PtrToStringAnsi((nint)text);
    }

    public static string? Wide(char* text)
    {
        if (text == null)
        {
            return null;
        }

        return new string(text);
    }
}
