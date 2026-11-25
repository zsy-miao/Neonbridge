// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using Xunit;

// The shim entry points reach the most recently created loader runtime, so tests that create one run one at a time.
[assembly: CollectionBehavior(DisableTestParallelization = true)]
