/*
 * C interface between PuTTY and the Rust engine crate.
 *
 * Threading: PuTTY keeps global state (timers, callbacks, socket tables), so
 * every function here runs on a single engine thread, the one inside
 * lc_engine_run(). The exception is lc_engine_wake(), callable from anywhere.
 */
#ifndef LINECON_SHIM_H
#define LINECON_SHIM_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct lc_session lc_session;

typedef enum {
    LC_EV_OUTPUT = 0,    /* data/len: bytes from the peer (arg: 1 if stderr) */
    LC_EV_STARTED,       /* authentication finished, session is live */
    LC_EV_ECHOEDIT,      /* arg: bit 0 echo, bit 1 line editing */
    LC_EV_PROMPT,        /* see lc_prompt_* accessors */
    LC_EV_HOSTKEY,       /* text: description, str1: key type, str2: key */
    LC_EV_WEAK_CRYPTO,   /* text: description of the weak primitive */
    LC_EV_LOG,           /* text: PuTTY event log line */
    LC_EV_FATAL,         /* text: connection failure reason */
    LC_EV_CLOSED,        /* arg: exit code or -1, text: reason or NULL */
    LC_EV_SENDOK,        /* backend is ready to accept input */
    LC_EV_SENT,          /* arg: bytes still buffered towards the peer */
} lc_event_kind;

typedef struct {
    lc_event_kind kind;
    void *user;
    const uint8_t *data;
    size_t len;
    int arg;
    const char *text;
    const char *str1;
    const char *str2;
    /* LC_EV_PROMPT: n_prompts entries in prompt_text/prompt_echo. */
    size_t n_prompts;
    const char *const *prompt_text;
    const unsigned char *prompt_echo;
} lc_event;

/* Host key store. check returns 0 match, 1 unknown, 2 changed. */
typedef struct {
    size_t (*on_event)(const lc_event *ev); /* LC_EV_OUTPUT: returns backlog */
    void (*on_drain)(void *ctx);            /* run queued engine commands */
    int (*hostkey_check)(void *ctx, const char *host, int port,
                         const char *keytype, const char *key);
    void (*hostkey_store)(void *ctx, const char *host, int port,
                          const char *keytype, const char *key);
    void *ctx;
} lc_callbacks;

typedef struct {
    const char *key;
    const char *value;
} lc_setting;

int lc_engine_init(const lc_callbacks *cb);
void lc_engine_run(void);  /* returns after lc_engine_stop() */
void lc_engine_stop(void);
void lc_engine_wake(void); /* thread-safe */

/* Settings use PuTTY's saved-session vocabulary (HostName, Protocol, ...). */
lc_session *lc_session_open(const lc_setting *settings, size_t n, void *user,
                            char **error_out);
void lc_session_send(lc_session *s, const void *data, size_t len);
void lc_session_size(lc_session *s, int cols, int rows);
int lc_session_special(lc_session *s, int code, int arg);
void lc_session_unthrottle(lc_session *s, size_t backlog);
void lc_session_answer_prompt(lc_session *s, const char *const *answers,
                              size_t n);
void lc_session_cancel_prompt(lc_session *s);
/* verdict: 0 reject, 1 accept once, 2 accept and store. */
void lc_session_answer_hostkey(lc_session *s, int verdict);
void lc_session_answer_weak(lc_session *s, int accept);
void lc_session_close(lc_session *s);

/* Special codes, mapped onto PuTTY's SessionSpecialCode. */
enum { LC_SPECIAL_BREAK = 1, LC_SPECIAL_EOF, LC_SPECIAL_PING, LC_SPECIAL_REKEY };

void lc_free_string(char *s);

/* Build identification, for version output and bug reports. */
const char *lc_putty_version(void);

#ifdef __cplusplus
}
#endif
#endif
