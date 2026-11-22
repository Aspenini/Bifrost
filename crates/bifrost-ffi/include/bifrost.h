#ifndef BIFROST_H
#define BIFROST_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct bifrost_session bifrost_session;

typedef int32_t bifrost_status;
enum {
    BIFROST_OK = 0,
    BIFROST_INVALID_ARGUMENT = 1,
    BIFROST_LOAD_FAILURE = 2,
    BIFROST_GUEST_FAILURE = 3,
    BIFROST_BUDGET_EXHAUSTED = 4,
    BIFROST_OUTPUT_TRUNCATED = 5,
    BIFROST_INTERNAL_ERROR = 6,
    BIFROST_UNAVAILABLE = 7
};

enum {
    BIFROST_CAP_KNOWN = 1,
    BIFROST_CAP_COMPILED = 2,
    BIFROST_CAP_IMPLEMENTED = 4,
    BIFROST_CAP_AVAILABLE = 8,
    BIFROST_CAP_VALIDATED = 16
};

typedef struct bifrost_run_result {
    uint32_t exit_code;
    size_t output_written;
} bifrost_run_result;

enum { BIFROST_X86_64 = 1, BIFROST_ARM64 = 2 };

typedef struct bifrost_raw_result {
    uint64_t pc;
    uint64_t return_register;
    uint64_t instructions;
} bifrost_raw_result;

uint32_t bifrost_abi_version(void);
bifrost_session *bifrost_create(void);
void bifrost_destroy(bifrost_session *session);
uint32_t bifrost_capability(const bifrost_session *session, const uint8_t *id, size_t len);
bifrost_status bifrost_run(const bifrost_session *session, const uint8_t *image,
                          size_t image_len, uint64_t instruction_limit, uint8_t *output,
                          size_t output_len, bifrost_run_result *result);
bifrost_status bifrost_run_raw(const bifrost_session *session, uint32_t isa,
                              const uint8_t *code, size_t len,
                              uint64_t instruction_limit, bifrost_raw_result *result);

#ifdef __cplusplus
}
#endif

#endif
