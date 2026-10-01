/*
 * brows12.h — C ABI for embedding the Brows12 engine (feature "capi").
 *
 * Build the static library:
 *   cargo build -p brows12-api --release --features capi
 *   # artifact: target/release/libbrows12_api.a
 *
 * Frame data is premultiplied RGBA8, row-major, top-left origin.
 * All pointers are borrowed unless a function explicitly hands ownership.
 */
#ifndef BROWS12_H
#define BROWS12_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct B12Engine B12Engine;
typedef struct B12Tab B12Tab;

/* Create an engine. `config_json` may be NULL; keys:
 *   {"viewport": [w, h], "user_agent": "...", "max_live_pages": n}
 * Free with b12_engine_free. */
B12Engine *b12_engine_new(const char *config_json);
void b12_engine_free(B12Engine *engine);

/* Tabs. Free with b12_tab_free. */
B12Tab *b12_tab_new(B12Engine *engine);
void b12_tab_free(B12Tab *tab);

/* Navigate. Returns 0 on success, 1 (bad handle) or 2 (load error). */
int b12_tab_load(B12Tab *tab, const char *url);

/* Latest framebuffer. Returns 0 when a frame exists. Pointers are valid
 * until the next load on the same tab. */
int b12_tab_frame(const B12Tab *tab, uint32_t *w, uint32_t *h,
                  const uint8_t **data, size_t *len);

/* Title. Pass buf=NULL to query the required capacity (incl. NUL). */
size_t b12_tab_title(const B12Tab *tab, uint8_t *buf, size_t cap);

#ifdef __cplusplus
}
#endif

#endif /* BROWS12_H */
