// Journal du runtime : toujours pour les messages du jeu, sur demande (ANYXBOXX_TRACE=1)
// pour le détail des appels. Écrit sur stderr, partagé avec AnyXboxX.
#pragma once

#include <windows.h>

#include <cstdarg>
#include <cstdio>

namespace anyxboxx {

inline bool TraceEnabled() {
  static const bool enabled = [] {
    char value[4] = {};
    const DWORD length = GetEnvironmentVariableA("ANYXBOXX_TRACE", value, sizeof value);
    return length == 1 && value[0] == '1';
  }();
  return enabled;
}

inline void Log(const char* format, ...) {
  va_list arguments;
  va_start(arguments, format);
  std::vfprintf(stderr, format, arguments);
  va_end(arguments);
  std::fputc('\n', stderr);
  std::fflush(stderr);
}

}  // namespace anyxboxx

#define ANYXBOXX_TRACE(...)                                   \
  do {                                                        \
    if (anyxboxx::TraceEnabled()) anyxboxx::Log(__VA_ARGS__); \
  } while (0)
