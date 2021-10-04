
#include <cstdio>

#include "../../tests/cpp/wasi_parallel.h"

void cpu_worker() {}

int main(int argc, char **argv)
{
    std::printf("main\n");
}

__attribute__((export_name("setup"))) void setup()
{
    std::printf("setup\n");
}

__attribute__((export_name("run"))) void run()
{
    std::printf("run\n");
}
