// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader;

/// <summary>
/// Marks a method that runs on every call the game makes at run time (XMem allocations, task queue submits and
/// dispatches, async operations). Such a method allocates nothing and never blocks: no lambda, LINQ, string building,
/// array, <c>new</c> of a reference type, lock or await; logging sits behind a level check in a separate method.
/// </summary>
[AttributeUsage(AttributeTargets.Method)]
internal sealed class HotPathAttribute : Attribute
{
}
