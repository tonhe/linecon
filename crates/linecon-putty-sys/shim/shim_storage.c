/*
 * Replacement for PuTTY's platform storage.c. Linking this first means the
 * registry on Windows and ~/.putty on Unix are never touched: settings arrive
 * as key/value pairs from the application, and host keys go to its store,
 * which is what lets the portable config travel with the binary.
 */
#include <stdlib.h>
#include <string.h>

#include "putty.h"
#include "storage.h"
#include "shim_internal.h"

struct settings_r {
    size_t n;
    char **keys;
    char **values;
};

settings_r *lc_settings_from_kv(const lc_setting *settings, size_t n)
{
    settings_r *sr = snew(settings_r);
    sr->n = n;
    sr->keys = snewn(n + 1, char *);
    sr->values = snewn(n + 1, char *);
    for (size_t i = 0; i < n; i++) {
        sr->keys[i] = dupstr(settings[i].key);
        sr->values[i] = dupstr(settings[i].value);
    }
    return sr;
}

void lc_settings_free(settings_r *sr)
{
    for (size_t i = 0; i < sr->n; i++) {
        sfree(sr->keys[i]);
        burnstr(sr->values[i]);
    }
    sfree(sr->keys);
    sfree(sr->values);
    sfree(sr);
}

static const char *sr_find(settings_r *sr, const char *key)
{
    if (!sr)
        return NULL;
    for (size_t i = 0; i < sr->n; i++)
        if (!strcmp(sr->keys[i], key))
            return sr->values[i];
    return NULL;
}

settings_r *open_settings_r(const char *sessionname) { return NULL; }
void close_settings_r(settings_r *handle) {}

char *read_setting_s(settings_r *handle, const char *key)
{
    const char *v = sr_find(handle, key);
    return v ? dupstr(v) : NULL;
}

int read_setting_i(settings_r *handle, const char *key, int defvalue)
{
    const char *v = sr_find(handle, key);
    return v ? atoi(v) : defvalue;
}

Filename *read_setting_filename(settings_r *handle, const char *key)
{
    const char *v = sr_find(handle, key);
    return v ? filename_from_str(v) : NULL;
}

FontSpec *read_setting_fontspec(settings_r *handle, const char *key)
{
    return NULL;
}

/* The application owns persistence, so PuTTY never writes settings. */
settings_w *open_settings_w(const char *sessionname, char **errmsg)
{
    *errmsg = dupstr("saving settings through PuTTY is not supported");
    return NULL;
}
void write_setting_s(settings_w *handle, const char *key, const char *value) {}
void write_setting_i(settings_w *handle, const char *key, int value) {}
void write_setting_filename(settings_w *handle, const char *key,
                            Filename *value) {}
void write_setting_fontspec(settings_w *handle, const char *key,
                            FontSpec *font) {}
void close_settings_w(settings_w *handle) {}
void del_settings(const char *sessionname) {}

settings_e *enum_settings_start(void) { return NULL; }
bool enum_settings_next(settings_e *handle, strbuf *out) { return false; }
void enum_settings_finish(settings_e *handle) {}

int check_stored_host_key(const char *hostname, int port, const char *keytype,
                          const char *key)
{
    return lc_shim_hostkey_check(hostname, port, keytype, key);
}

bool have_ssh_host_key(const char *hostname, int port, const char *keytype)
{
    return lc_shim_hostkey_check(hostname, port, keytype, "") != 1;
}

void store_host_key(Seat *seat, const char *hostname, int port,
                    const char *keytype, const char *key)
{
    lc_shim_hostkey_store(hostname, port, keytype, key);
}

host_ca_enum *enum_host_ca_start(void) { return NULL; }
bool enum_host_ca_next(host_ca_enum *handle, strbuf *out) { return false; }
void enum_host_ca_finish(host_ca_enum *handle) {}
host_ca *host_ca_load(const char *name) { return NULL; }
char *host_ca_save(host_ca *hca)
{
    return dupstr("host CAs are not supported yet");
}
char *host_ca_delete(const char *name)
{
    return dupstr("host CAs are not supported yet");
}

/* Entropy comes from the OS on every start, so no seed file is kept. */
void read_random_seed(noise_consumer_t consumer) {}
void write_random_seed(void *data, int len) {}

void cleanup_all(void) {}

#ifdef _WIN32
/* PuTTY's settings loader updates the Windows jump list; we keep none. */
void add_session_to_jumplist(const char *sessionname) {}
void remove_session_from_jumplist(const char *sessionname) {}
#endif
