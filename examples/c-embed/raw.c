#include "bifrost.h"
#include <stdio.h>

int main(void) {
    static const uint8_t x86[] = {0x48, 0xb8, 42, 0, 0, 0, 0, 0, 0, 0};
    static const uint8_t arm[] = {0x60, 0x05, 0x80, 0xd2};
    static const uint8_t loop[] = {0xeb, 0xfe};
    bifrost_raw_result result;

    if (bifrost_abi_version() != 1) return 1;
    bifrost_session *session = bifrost_create();
    if (!session) return 2;
    if (bifrost_run_raw(session, BIFROST_X86_64, x86, sizeof(x86), 1, &result) != BIFROST_OK
        || result.return_register != 42 || result.instructions != 1
        || bifrost_run_raw(session, BIFROST_ARM64, arm, sizeof(arm), 1, &result) != BIFROST_OK
        || result.return_register != 43 || result.instructions != 1
        || bifrost_run_raw(session, BIFROST_X86_64, loop, sizeof(loop), 4, &result) != BIFROST_BUDGET_EXHAUSTED
        || result.instructions != 4
        || bifrost_run_raw(session, BIFROST_X86_64, x86, 2, 1, &result) != BIFROST_GUEST_FAILURE
        || result.instructions != 0) {
        bifrost_destroy(session);
        return 3;
    }
    bifrost_destroy(session);
    puts("Raw C embedding passed on both guest ISAs.");
    return 0;
}
