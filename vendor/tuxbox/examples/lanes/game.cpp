#include "gpu.h"
#include "game.h"
#include "input.h"
#include "screen.h"
#include "selftest.h"
extern "C" {
#include "../../runtime/xmem/xmem.h"
}
#include <algorithm>
#include <cstdio>
#include <cwchar>
#include <filesystem>
#include <fstream>
#include <memory>
#include <new>
#include <stdexcept>
#include <string>

namespace {
constexpr std::uint32_t replay_seed = 0x5194c0d1u;

void expect(bool ok, const char* reason) {
    if (!ok) throw std::runtime_error(reason);
}

void replay(Game& game, Screen& screen) {
    lanes_input_replay_check();
    screen.verify(game);
    game.move(1);
    screen.verify(game);
    game.restart();
    // Golden rows for this seed, in order. This also checks unvisited hazards from a winning playthrough.
    constexpr Course reference{
        0x0001, 0x0102, 0x0201, 0x0100,
        0x0001, 0x0102, 0x0201, 0x0100,
        0x0001, 0x0002, 0x0201, 0x0200,
        0x0001, 0x0002, 0x0201, 0x0200,
    };
    expect(game.rows == reference, "course differs from the fixed-seed reference");
    constexpr int moves[] = {0, 1, -1, -1, 1, 1, -1, -1, 1, 1, -1, -1, 1, 1, -1, -1};
    for (int move : moves) game.move(move);
    expect(game.finish == Finish::won && game.score == 16 && game.turn == course_length, "winning replay differs");
    game.move(1);
    expect(game.turn == course_length && game.score == 16, "won game accepted another move");

    game.restart();
    int coin = game.rows.front() & 255, hazard = game.rows.front() >> 8;
    int safe = 3 - coin - hazard;
    game.move(safe - game.lane);
    expect(game.finish == Finish::playing && game.turn == 1 && game.score == 0, "safe miss awarded a point");
    game.restart();
    game.move(hazard - game.lane);
    expect(game.finish == Finish::crashed && game.turn == 1 && game.score == 0, "collision failed to end play");
    game.move(0);
    expect(game.turn == 1 && game.score == 0, "crashed game accepted another move");
}

void play(Game& game, Screen& screen, LanesInput& controls) {
    int x = Screen::width / 2;
    int y = Screen::height / 2;
    bool cursor_visible = false;
    bool dirty = true;
    while (screen.draw(game, x, y, cursor_visible, dirty)) {
        const auto input = controls.poll();
        if (input.quit) return;
        const bool pointer = input.mouse_dx || input.mouse_dy || input.click;
        const bool keys = input.restart || input.left || input.right || input.hold;
        dirty = pointer || keys;
        if (pointer) {
            x = static_cast<int>(std::clamp<long long>(static_cast<long long>(x) + input.mouse_dx, 0, Screen::width - 1));
            y = static_cast<int>(std::clamp<long long>(static_cast<long long>(y) + input.mouse_dy, 0, Screen::height - 1));
            cursor_visible = true;
        } else if (keys) {
            cursor_visible = false;
        }
        if (input.restart) game.restart();
        else if (input.hold) game.move(0);
        else if (input.left != input.right) game.move(input.left ? -1 : 1);
        else if (input.click) {
            const int target = Screen::lane_at(x, y);
            if (target >= 0) {
                if (game.finish == Finish::playing) game.move(std::clamp(target - game.lane, -1, 1));
                else game.restart();
            }
        }
    }
}
}

int wmain(int argc, wchar_t** argv) {
    bool scripted = false;
    const wchar_t* report = nullptr;
    const wchar_t* adapter = nullptr;
    const wchar_t* selftest = nullptr;
    bool benchmark = false;
    for (int i = 1; i < argc; ++i) {
        if (!std::wcscmp(argv[i], L"--replay") && !scripted) scripted = true;
        else if (!std::wcscmp(argv[i], L"--report") && !report && i + 1 < argc) report = argv[++i];
        else if (!std::wcscmp(argv[i], L"--adapter") && !adapter && i + 1 < argc) adapter = argv[++i];
        else if (!std::wcscmp(argv[i], L"--benchmark") && !benchmark) benchmark = true;
        else if (!std::wcscmp(argv[i], L"--selftest") && !selftest && i + 1 < argc) selftest = argv[++i];
        else {
            std::fputs("usage: lanes.exe [--adapter VVVV:DDDD] [--replay | --selftest NAME|all | --benchmark] [--report FILE]\n",
                       stderr);
            return 2;
        }
    }
    if (int(scripted) + int(selftest != nullptr) + int(benchmark) > 1) return 2;
    if (report && !scripted && !selftest && !benchmark) return 2;
    if (selftest || benchmark) {
        std::string line;
        int status = 0;
        try {
            line = benchmark ? run_benchmark(adapter) : run_selftest(selftest, adapter);
        } catch (const std::exception& error) {
            line = std::string(benchmark ? "lanes-benchmark FAIL " : "lanes-selftest FAIL ") + error.what();
            status = 1;
        }
        std::puts(line.c_str());
        if (report) {
            std::ofstream proof(std::filesystem::path(report), std::ios::binary | std::ios::trunc);
            proof << line << '\n';
            if (!proof) return 1;
        }
        std::fflush(nullptr);
        // A failed scenario can leave queues that never finish. Do not wait for
        // them while releasing devices.
        if (status) TerminateProcess(GetCurrentProcess(), static_cast<UINT>(status));
        return status;
    }
    try {
        std::ofstream proof;
        if (report) {
            proof.open(std::filesystem::path(report), std::ios::binary | std::ios::trunc);
            expect(bool(proof), "cannot open replay report");
        }
        auto release = [](Game* p) { p->~Game(); XMemFreeDefault(p, XMEM_ORDINARY_ATTRIBUTES); };
        void* block = XMemAllocDefault(sizeof(Game), XMEM_ORDINARY_ATTRIBUTES);
        expect(block != nullptr, "XMem board allocation failed");
        std::unique_ptr<Game, decltype(release)> game(new(block) Game{}, release);
        std::string pci;
        auto device = create_lanes_device(adapter, pci);
        generate_course(device.Get(), game->rows, scripted ? replay_seed : GetTickCount());
        Screen screen(device.Get());
        LanesInput controls;
        if (!scripted) { play(*game, screen, controls); return 0; }
        controls.poll();
        replay(*game, screen);
        game.reset();
        const auto line = "lanes-replay PASS pci=" + pci + " course=16/16 win=16/16 safe-miss=1 collision=1 frames=2 input=gameinput";
        if (report) {
            proof << line << '\n';
            proof.close();
            expect(bool(proof), "cannot write replay report");
        }
        std::puts(line.c_str());
        return 0;
    } catch (const std::exception& error) {
        std::fprintf(stderr, "lanes: %s\n", error.what());
        return 1;
    }
}
