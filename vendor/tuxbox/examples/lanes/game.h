#pragma once

#include "course.h"
#include <algorithm>

enum class Finish { playing, won, crashed };

struct Game {
    Course rows{};
    int lane = 1, score = 0;
    unsigned turn = 0;
    Finish finish = Finish::playing;

    void restart() {
        lane = 1;
        score = 0;
        turn = 0;
        finish = Finish::playing;
    }

    void move(int direction) {
        if (finish != Finish::playing) return;
        lane = std::clamp(lane + direction, 0, 2);
        const auto row = rows[turn++];
        if (unsigned(lane) == (row >> 8)) {
            finish = Finish::crashed;
            return;
        }
        if (unsigned(lane) == (row & 255u)) ++score;
        if (turn == rows.size()) finish = Finish::won;
    }
};
