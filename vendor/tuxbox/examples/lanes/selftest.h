#pragma once

#include <string>

// Headless checks of Xbox graphics behaviour that retail titles depend on.
// Each scenario runs on its own device and fails, rather than hangs, when the
// GPU does not finish in time.

// The scenario names, in the order `all` runs them.
extern const wchar_t* const selftest_scenarios[];
extern const unsigned selftest_scenario_count;

// Run one scenario by name, or every one for L"all". Returns the receipt line;
// throws with the failing scenario and step.
std::string run_selftest(const wchar_t* scenario, const wchar_t* adapter);

// Time a fixed set of operations on the same device layout. Returns the receipt
// line with one `name=value` reading per operation.
std::string run_benchmark(const wchar_t* adapter);
