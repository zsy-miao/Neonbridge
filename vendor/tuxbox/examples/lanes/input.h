#pragma once
#include <memory>

struct InputFrame {
    bool left{}, right{}, hold{}, restart{}, quit{}, click{};
    int mouse_dx{}, mouse_dy{};
};

class LanesInput {
public:
    LanesInput();
    ~LanesInput();
    InputFrame poll();

private:
    struct State;
    std::unique_ptr<State> state;
};

void lanes_input_replay_check();
