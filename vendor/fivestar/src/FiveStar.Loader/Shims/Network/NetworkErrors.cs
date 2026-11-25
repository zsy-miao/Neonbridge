// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Network;

/// <summary>The errors a sealed network call reports (winerror.h, winhttp.h).</summary>
internal static class NetworkErrors
{
    /// <summary>Winsock's SOCKET_ERROR return value.</summary>
    public const int SocketError = -1;

    /// <summary>WSAHOST_NOT_FOUND: no such host is known.</summary>
    public const int HostNotFound = 11001;

    /// <summary>WSAENETUNREACH: the network cannot be reached from this host.</summary>
    public const int NetworkUnreachable = 10051;

    /// <summary>WSAENETDOWN: the network subsystem is down.</summary>
    public const int NetworkDown = 10050;

    /// <summary>WSAEOPNOTSUPP.</summary>
    public const int OperationNotSupported = 10045;

    /// <summary>ERROR_WINHTTP_NAME_NOT_RESOLVED.</summary>
    public const int NameNotResolved = 12007;

    /// <summary>ERROR_WINHTTP_CANNOT_CONNECT.</summary>
    public const int CannotConnect = 12029;

    /// <summary>HRESULT_FROM_WIN32(ERROR_WINHTTP_NAME_NOT_RESOLVED), the error libHttpClient reports for an unknown host.</summary>
    public const int NameNotResolvedResult = unchecked((int)0x80072EE7);
}
