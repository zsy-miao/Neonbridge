#pragma once
#include <memory>

struct ID3D12Device;
struct Game;

class Screen {
  public:
    static constexpr int width = 520, height = 640;
    static int lane_at(int x, int y);
    explicit Screen(ID3D12Device* device);
    ~Screen();
    bool draw(const Game& game, int cursor_x, int cursor_y, bool cursor_visible, bool dirty);
    void verify(const Game& game);

  private:
    struct Impl;
    std::unique_ptr<Impl> state_;
};
