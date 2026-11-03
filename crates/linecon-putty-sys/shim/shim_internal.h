#ifndef LINECON_SHIM_INTERNAL_H
#define LINECON_SHIM_INTERNAL_H

#include "linecon_shim.h"
#include "putty.h"
#include "storage.h"

settings_r *lc_settings_from_kv(const lc_setting *settings, size_t n);
void lc_settings_free(settings_r *sr);

int lc_shim_hostkey_check(const char *host, int port, const char *keytype,
                          const char *key);
void lc_shim_hostkey_store(const char *host, int port, const char *keytype,
                           const char *key);

#endif
