#include "ai_pow_gemm.h"

#include <cstdint>
#include <cstdio>
#include <random>
#include <vector>

static void cpu_state(const std::vector<int8_t>& a, const std::vector<int8_t>& b,
                      uint32_t h, uint32_t w, uint32_t k, uint32_t rank,
                      uint32_t dot, int32_t out[16]) {
  std::vector<int32_t> accum(h * w, 0);
  for (int i = 0; i < 16; ++i) out[i] = 0;
  for (uint32_t step = 0; step < dot / rank; ++step) {
    for (uint32_t row = 0; row < h; ++row) {
      for (uint32_t col = 0; col < w; ++col) {
        int32_t delta = 0;
        for (uint32_t x = 0; x < rank; ++x) {
          const uint32_t index = step * rank + x;
          delta += int32_t(a[row * k + index]) * int32_t(b[col * k + index]);
        }
        accum[row * w + col] += delta;
      }
    }
    int32_t folded = 0;
    for (int32_t value : accum) folded ^= value;
    const uint32_t slot = step & 15;
    const uint32_t prior = static_cast<uint32_t>(out[slot]);
    out[slot] = static_cast<int32_t>((prior << 13 | prior >> 19) ^
                                     static_cast<uint32_t>(folded));
  }
}

int main() {
  std::mt19937 rng(0x4e4f434b);
  for (uint32_t trial = 0; trial < 1000; ++trial) {
    const uint32_t h = 1 + rng() % 16;
    const uint32_t w = 1 + rng() % 16;
    const uint32_t rank = 1u << (rng() % 7);
    const uint32_t steps = 1 + rng() % 32;
    const uint32_t k = rank * steps;
    std::vector<int8_t> a(h * k), b(w * k);
    for (int8_t& value : a) value = static_cast<int8_t>(int(rng() % 255) - 127);
    for (int8_t& value : b) value = static_cast<int8_t>(int(rng() % 255) - 127);
    int32_t expected[16], actual[16];
    cpu_state(a, b, h, w, k, rank, k, expected);
    const int status = ai_pow_cuda_tile_state(
        a.data(), b.data(), h, w, k, rank, k, actual, nullptr);
    if (status != 0) {
      std::fprintf(stderr, "CUDA error %d at trial %u\n", status, trial);
      return 1;
    }
    for (int slot = 0; slot < 16; ++slot) {
      if (expected[slot] != actual[slot]) {
        std::fprintf(stderr,
                     "mismatch trial=%u slot=%d expected=%08x actual=%08x\n",
                     trial, slot, uint32_t(expected[slot]), uint32_t(actual[slot]));
        return 1;
      }
    }
  }
  {
    constexpr uint32_t h = 16;
    constexpr uint32_t w = 16;
    constexpr uint32_t rank = 128;
    constexpr uint32_t steps = 32;
    constexpr uint32_t k = rank * steps;
    std::vector<int8_t> a(h * k), b(w * k);
    for (size_t index = 0; index < a.size(); ++index) {
      a[index] = static_cast<int8_t>((index * 73 + index / k * 19) % 255 - 127);
    }
    for (size_t index = 0; index < b.size(); ++index) {
      b[index] = static_cast<int8_t>((index * 151 + index / k * 31) % 255 - 127);
    }
    int32_t expected[16], actual[16];
    cpu_state(a, b, h, w, k, rank, k, expected);
    const int status = ai_pow_cuda_tile_state(
        a.data(), b.data(), h, w, k, rank, k, actual, nullptr);
    if (status != 0) {
      std::fprintf(stderr, "CUDA error %d in accumulation stress case\n", status);
      return 1;
    }
    for (int slot = 0; slot < 16; ++slot) {
      if (expected[slot] != actual[slot]) {
        std::fprintf(stderr,
                     "stress mismatch slot=%d expected=%08x actual=%08x\n",
                     slot, uint32_t(expected[slot]), uint32_t(actual[slot]));
        return 1;
      }
    }
  }
  std::puts("1000 randomized Pearl tile-state differentials passed");
  return 0;
}
