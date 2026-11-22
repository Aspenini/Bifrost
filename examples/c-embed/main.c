#include "bifrost.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

int main(int argc, char **argv) {
    static const uint8_t capability[] = "linux-elf";
    static const char expected[] = "Hello from Bifrost ELF!\n";
    uint8_t image[65536], output[256];
    bifrost_run_result result;

    if (argc != 2 || bifrost_abi_version() != 1) return 1;
    FILE *file = NULL;
#ifdef _WIN32
    if (fopen_s(&file, argv[1], "rb") != 0) return 2;
#else
    file = fopen(argv[1], "rb");
#endif
    if (!file) return 2;
    size_t length = fread(image, 1, sizeof(image), file);
    int failed = ferror(file);
    fclose(file);
    if (failed || length == sizeof(image)) return 3;

    bifrost_session *session = bifrost_create();
    if (!session) return 4;
    if (!(bifrost_capability(session, capability, sizeof(capability) - 1) & BIFROST_CAP_AVAILABLE)) {
        bifrost_destroy(session);
        return 5;
    }
    bifrost_status status = bifrost_run(session, image, length, 10000, output, sizeof(output), &result);
    bifrost_destroy(session);
    if (status != BIFROST_OK || result.exit_code != 42 || result.output_written != sizeof(expected) - 1
        || memcmp(output, expected, sizeof(expected) - 1) != 0) return 6;
    return fwrite(output, 1, result.output_written, stdout) == result.output_written ? 0 : 7;
}
