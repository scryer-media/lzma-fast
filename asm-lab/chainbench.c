// Latency of the candidate LZMA range-decoder recurrences, in isolation.
//
// Each kernel runs the loop-carried arithmetic of one binary decode with the
// data-dependent parts (prob load, prob update, symbol tree) removed, so what
// is measured is the cycles/iteration of the recurrence itself.
//
//   clang -O2 -o chainbench chainbench.c && ./chainbench
#include <stdio.h>
#include <time.h>

#define N 200000000L

static double secs(void (*f)(long)) {
    struct timespec a, b;
    f(1000000);                       // warm up / settle the clock
    clock_gettime(CLOCK_MONOTONIC, &a);
    f(N);
    clock_gettime(CLOCK_MONOTONIC, &b);
    return (b.tv_sec - a.tv_sec) + (b.tv_nsec - a.tv_nsec) / 1e9;
}

// stock: csel(range) -> lsr -> mul -> subs
__attribute__((noinline)) static void k_stock(long n) {
    register unsigned r asm("w20") = 0xFF000000u;
    register unsigned c asm("w21") = 0x7F000000u;
    register unsigned p asm("w22") = 1024;
    __asm__ volatile(
        "1:\n"
        "  lsr  w7, %w[r], #11\n"
        "  mul  w7, w7, %w[p]\n"
        "  subs w6, %w[c], w7\n"
        "  sub  w8, %w[r], w7\n"
        "  csel %w[r], w7, w8, lo\n"
        "  orr  %w[r], %w[r], #0xFF000000\n"   // keep range normalised
        "  subs %[n], %[n], #1\n"
        "  b.ne 1b\n"
        : [r] "+r"(r), [n] "+r"(n)
        : [c] "r"(c), [p] "r"(p)
        : "w6", "w7", "w8", "cc");
}

// msub + speculative shift: csel(rangeA) -> mul -> subs
__attribute__((noinline)) static void k_msub(long n) {
    register unsigned r asm("w20") = 0xFF000000u;
    register unsigned a asm("w23") = 0xFF000000u >> 11;
    register unsigned c asm("w21") = 0x7F000000u;
    register unsigned p asm("w22") = 1024;
    __asm__ volatile(
        "1:\n"
        "  mul  w7, %w[a], %w[p]\n"
        "  msub w8, %w[a], %w[p], %w[r]\n"
        "  subs w6, %w[c], w7\n"
        "  lsr  w9, w7, #11\n"
        "  lsr  w10, w8, #11\n"
        "  csel %w[a], w9, w10, lo\n"
        "  csel %w[r], w7, w8, lo\n"
        "  orr  %w[r], %w[r], #0xFF000000\n"
        "  orr  %w[a], %w[a], #0x001F0000\n"
        "  subs %[n], %[n], #1\n"
        "  b.ne 1b\n"
        : [r] "+r"(r), [a] "+r"(a), [n] "+r"(n)
        : [c] "r"(c), [p] "r"(p)
        : "w6", "w7", "w8", "w9", "w10", "cc");
}

// isolated latencies
__attribute__((noinline)) static void k_mul(long n) {
    register unsigned x asm("w20") = 3;
    __asm__ volatile("1: mul %w[x], %w[x], %w[x]\n subs %[n],%[n],#1\n b.ne 1b\n"
                     : [x] "+r"(x), [n] "+r"(n) :: "cc");
}
__attribute__((noinline)) static void k_msub_lat(long n) {
    register unsigned x asm("w20") = 3;
    register unsigned y asm("w21") = 5;
    __asm__ volatile("1: msub %w[x], %w[y], %w[y], %w[x]\n subs %[n],%[n],#1\n b.ne 1b\n"
                     : [x] "+r"(x), [n] "+r"(n) : [y] "r"(y) : "cc");
}
__attribute__((noinline)) static void k_muladd_lat(long n) {
    // latency from the ADDEND operand of MSUB (what the range chain uses)
    register unsigned x asm("w20") = 3;
    register unsigned y asm("w21") = 5;
    __asm__ volatile("1: madd %w[x], %w[y], %w[y], %w[x]\n subs %[n],%[n],#1\n b.ne 1b\n"
                     : [x] "+r"(x), [n] "+r"(n) : [y] "r"(y) : "cc");
}
__attribute__((noinline)) static void k_csel(long n) {
    register unsigned x asm("w20") = 3;
    __asm__ volatile("1: csel %w[x], %w[x], %w[x], eq\n subs %[n],%[n],#1\n b.ne 1b\n"
                     : [x] "+r"(x), [n] "+r"(n) :: "cc");
}
__attribute__((noinline)) static void k_add(long n) {
    register unsigned x asm("w20") = 3;
    __asm__ volatile("1: add %w[x], %w[x], #1\n subs %[n],%[n],#1\n b.ne 1b\n"
                     : [x] "+r"(x), [n] "+r"(n) :: "cc");
}
// load-to-use latency: ldrh with a register-offset, chained through the index
__attribute__((noinline)) static void k_ldrh(long n) {
    static unsigned short tbl[16] __attribute__((aligned(64))) = {0};
    register unsigned long x asm("w20") = 0;
    register unsigned long b asm("x21") = (unsigned long)tbl;
    __asm__ volatile("1: ldrh %w[x], [%[b], %[x], lsl #1]\n subs %[n],%[n],#1\n b.ne 1b\n"
                     : [x] "+r"(x), [n] "+r"(n) : [b] "r"(b) : "cc");
}

int main(void) {
    struct { const char *n; void (*f)(long); } ks[] = {
        {"add        (latency 1 ref)", k_add},
        {"csel", k_csel},
        {"mul  (x=x*x)", k_mul},
        {"madd (addend->result)", k_muladd_lat},
        {"msub (addend->result)", k_msub_lat},
        {"ldrh [b, x, lsl#1]", k_ldrh},
        {"CHAIN stock", k_stock},
        {"CHAIN msub+specshift", k_msub},
    };
    double ghz = 0;
    for (unsigned i = 0; i < sizeof ks / sizeof *ks; i++) {
        double s = secs(ks[i].f);
        double cyc = s * 4.21e9 / N;   // clock measured separately
        if (i == 0) ghz = 1 / (s / N) / 1e9;
        printf("%-28s %6.3f s  %5.2f cycles/iter (assuming 4.21 GHz; "
               "add-loop says %.2f GHz)\n", ks[i].n, s, cyc, ghz);
    }
    return 0;
}
