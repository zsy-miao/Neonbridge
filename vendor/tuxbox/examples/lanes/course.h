#pragma once

#include <array>
#include <cstdint>

struct ID3D12Device;

constexpr unsigned course_length = 16;
using Course = std::array<std::uint32_t, course_length>;

void generate_course(ID3D12Device* device, Course& rows, std::uint32_t seed);
