#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <inttypes.h>
#include <stdio.h>
#include <wchar.h>

#if !defined(_M_X64) && !defined(__x86_64__)
#error Build this fixture for x86-64
#endif

/* The two collectible tiles carry independent bits. The outside walls keep
   each one-step neighbor lookup inside the array. */
static const char board[][8] = {
    "#######",
    "#S.1..#",
    "#.#.#.#",
    "#2...E#",
    "#######"
};

typedef struct {
    unsigned x, y, coin_mask, ticks, bumps;
} State;

static int advance(State *state, wchar_t move) {
    unsigned x = state->x, y = state->y;
    switch (move) {
    case L'L': --x; break;
    case L'R': ++x; break;
    case L'U': --y; break;
    case L'D': ++y; break;
    default: return 0;
    }

    char tile = board[y][x];
    ++state->ticks;
    if (tile == '#') {
        ++state->bumps;
    } else {
        state->x = x;
        state->y = y;
        if (tile == '1' || tile == '2')
            state->coin_mask |= 1u << (tile - '1');
    }
    return 1;
}

static int write_receipt(const wchar_t *path, const char *line, int length) {
    HANDLE out = path == NULL ? GetStdHandle(STD_OUTPUT_HANDLE)
                              : CreateFileW(path, GENERIC_WRITE, 0, NULL, CREATE_NEW,
                                            FILE_ATTRIBUTE_NORMAL, NULL);
    if (!out || out == INVALID_HANDLE_VALUE)
        return 3;
    DWORD written = 0;
    BOOL ok = WriteFile(out, line, (DWORD)length, &written, NULL);
    if (path && !CloseHandle(out))
        return 3;
    return ok && written == (DWORD)length ? 0 : 3;
}

static int play_route(const wchar_t *route, const wchar_t *path) {
    State state = {1, 1, 0, 0, 0};
    LARGE_INTEGER frequency, start, end;
    if (!QueryPerformanceFrequency(&frequency) || frequency.QuadPart <= 0 ||
        !QueryPerformanceCounter(&start))
        return 3;
    for (const wchar_t *move = route; *move; ++move) {
        if (!advance(&state, *move)) {
            fputs("invalid move\n", stderr);
            return 2;
        }
    }
    if (!QueryPerformanceCounter(&end) || end.QuadPart < start.QuadPart)
        return 3;
    char line[100];
    int length = snprintf(line, sizeof(line),
        "route-game x=%u y=%u coin_mask=%u ticks=%u bumps=%u exit=%d\n",
        state.x, state.y, state.coin_mask, state.ticks, state.bumps, board[state.y][state.x] == 'E');
    return length > 0 && (size_t)length < sizeof(line) ? write_receipt(path, line, length) : 3;
}

static int benchmark(const wchar_t *path) {
    enum { bank_size = 256, route_length = 128, laps = 256 };
    static const wchar_t directions[] = L"LRUD";
    wchar_t routes[bank_size][route_length];
    uint32_t word = UINT32_C(0x54555842);
    for (unsigned i = 0; i < bank_size; ++i) {
        for (unsigned j = 0; j < route_length; ++j) {
            word ^= word << 13;
            word ^= word >> 17;
            word ^= word << 5;
            routes[i][j] = directions[word >> 30];
        }
    }

    uint64_t digest = UINT64_C(0xcbf29ce484222325);
    unsigned finished = 0, moves = 0;
    LARGE_INTEGER frequency, start, end;
    if (!QueryPerformanceFrequency(&frequency) || frequency.QuadPart <= 0 ||
        !QueryPerformanceCounter(&start))
        return 3;
    for (unsigned lap = 0; lap < laps; ++lap) {
        for (unsigned i = 0; i < bank_size; ++i) {
            State state = {1, 1, 0, 0, 0};
            for (unsigned j = 0; j < route_length; ++j) {
                unsigned offset = (lap + i + j) % route_length;
                advance(&state, routes[i][offset]);
            }
            unsigned exit_tile = board[state.y][state.x] == 'E';
            unsigned packed = state.x | state.y << 3 | state.coin_mask << 6 |
                              state.bumps << 8 | exit_tile << 16;
            digest = (digest ^ packed) * UINT64_C(0x100000001b3);
            ++finished;
            moves += state.ticks;
        }
    }
    if (!QueryPerformanceCounter(&end) || end.QuadPart <= start.QuadPart)
        return 3;
    if (finished != bank_size * laps || moves != bank_size * laps * route_length ||
        digest != UINT64_C(0x24316bf89788468d)) {
        fputs("replay benchmark result differs from reference\n", stderr);
        return 4;
    }
    char line[160];
    int length = snprintf(line, sizeof(line),
        "route-bench routes=%u moves=%u checksum=%016" PRIx64 " elapsed_ticks=%" PRId64
        " frequency=%" PRId64 "\n", finished, moves, digest, (int64_t)(end.QuadPart - start.QuadPart),
        (int64_t)frequency.QuadPart);
    return length > 0 && (size_t)length < sizeof(line) ? write_receipt(path, line, length) : 3;
}

int wmain(int argc, wchar_t **argv) {
    if ((argc != 2 && argc != 3) || !argv[1][0] || wcslen(argv[1]) > 256) {
        fputs("usage: route-game ROUTE|--bench [NEW_RESULT_FILE]\n", stderr);
        return 2;
    }
    const wchar_t *path = argc == 3 ? argv[2] : NULL;
    return wcscmp(argv[1], L"--bench") == 0 ? benchmark(path) : play_route(argv[1], path);
}
