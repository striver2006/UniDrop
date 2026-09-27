// UniClip Rust 核心的 FFI 符号声明（与 native/src/lib.rs 的 #[no_mangle] 一一对应）
#ifndef UNIDROP_MOBILE_H
#define UNIDROP_MOBILE_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef void (*unidrop_event_callback)(uintptr_t user_data, const char *event_json);

void unidrop_free_string(char *ptr);
void unidrop_set_event_callback(unidrop_event_callback cb, uintptr_t user_data);
char *unidrop_start(const char *config_json);
void unidrop_stop(void);
char *unidrop_invoke(const char *cmd_json);

#ifdef __cplusplus
}
#endif

#endif // UNIDROP_MOBILE_H
