/*
 * Seat, LogPolicy and event loop glue around PuTTY's backends.
 *
 * Nothing here touches the wire. PuTTY owns the protocols; this file only
 * turns its callbacks into events and its async prompts into answers that
 * arrive later from the application.
 */
#include <assert.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "putty.h"
#include "ssh.h"
#include "storage.h"

#ifndef _WIN32
#include <errno.h>
#include <fcntl.h>
#include <unistd.h>
#endif

#include "linecon_shim.h"
#include "shim_internal.h"

struct lc_session {
    Seat seat;
    LogPolicy logpolicy;
    void *user;
    Conf *conf;
    LogContext *logctx;
    Backend *backend;
    Ldisc *ldisc;
    lc_session *next;

    bufchain pending_input;
    bool sendok_seen;
    bool closed_reported;
    char *fatal_message;

    prompts_t *prompt;
    SeatPromptResult prompt_result;
    bool prompt_answered;

    void (*dialog_cb)(void *ctx, SeatPromptResult result);
    void *dialog_ctx;
    char *hk_host;
    int hk_port;
    char *hk_type;
    char *hk_key;
};

static lc_callbacks g_cb;
static lc_session *g_sessions;
static volatile bool g_stop;

/* Filled in around each host key dialog so store_host_key can find its ctx. */
void *lc_shim_cb_ctx(void) { return g_cb.ctx; }

int lc_shim_hostkey_check(const char *host, int port, const char *keytype,
                          const char *key)
{
    return g_cb.hostkey_check(g_cb.ctx, host, port, keytype, key);
}

void lc_shim_hostkey_store(const char *host, int port, const char *keytype,
                           const char *key)
{
    g_cb.hostkey_store(g_cb.ctx, host, port, keytype, key);
}

static size_t emit(const lc_event *ev) { return g_cb.on_event(ev); }

static void emit_text(lc_session *s, lc_event_kind kind, int arg,
                      const char *text)
{
    lc_event ev;
    memset(&ev, 0, sizeof(ev));
    ev.kind = kind;
    ev.user = s->user;
    ev.arg = arg;
    ev.text = text;
    emit(&ev);
}

/* ---------------------------------------------------------------- Seat -- */

static lc_session *seat_session(Seat *seat)
{
    return container_of(seat, lc_session, seat);
}

static size_t shim_output(Seat *seat, SeatOutputType type, const void *data,
                          size_t len)
{
    lc_session *s = seat_session(seat);
    lc_event ev;
    memset(&ev, 0, sizeof(ev));
    ev.kind = LC_EV_OUTPUT;
    ev.user = s->user;
    ev.data = data;
    ev.len = len;
    ev.arg = (type == SEAT_OUTPUT_STDERR) ? 1 : 0;
    return emit(&ev);
}

static bool shim_eof(Seat *seat) { return true; }

static void shim_sent(Seat *seat, size_t new_sendbuffer)
{
    emit_text(seat_session(seat), LC_EV_SENT, (int)new_sendbuffer, NULL);
}

static size_t shim_banner(Seat *seat, const void *data, size_t len)
{
    return shim_output(seat, SEAT_OUTPUT_STDERR, data, len);
}

static SeatPromptResult shim_get_userpass_input(Seat *seat, prompts_t *p)
{
    lc_session *s = seat_session(seat);

    if (s->prompt == p) {
        /* PuTTY came back after our callback: hand over the final result. */
        if (!s->prompt_answered)
            return SPR_INCOMPLETE;
        s->prompt = NULL;
        s->prompt_answered = false;
        return s->prompt_result;
    }

    size_t n = p->n_prompts;
    const char **texts = snewn(n + 1, const char *);
    unsigned char *echoes = snewn(n + 1, unsigned char);
    for (size_t i = 0; i < n; i++) {
        texts[i] = p->prompts[i]->prompt;
        echoes[i] = p->prompts[i]->echo ? 1 : 0;
    }

    s->prompt = p;
    s->prompt_answered = false;

    lc_event ev;
    memset(&ev, 0, sizeof(ev));
    ev.kind = LC_EV_PROMPT;
    ev.user = s->user;
    ev.text = p->instruction;
    ev.str1 = p->name;
    ev.arg = p->from_server ? 1 : 0;
    ev.n_prompts = n;
    ev.prompt_text = texts;
    ev.prompt_echo = echoes;
    emit(&ev);

    sfree(texts);
    sfree(echoes);
    return SPR_INCOMPLETE;
}

static void shim_notify_session_started(Seat *seat)
{
    emit_text(seat_session(seat), LC_EV_STARTED, 0, NULL);
}

static void shim_connection_fatal(Seat *seat, const char *message)
{
    lc_session *s = seat_session(seat);
    /* Any prompt in flight dies with the connection. */
    s->prompt = NULL;
    s->dialog_cb = NULL;
    if (!s->fatal_message)
        s->fatal_message = dupstr(message);
    emit_text(s, LC_EV_FATAL, 0, message);
}

static void shim_nonfatal(Seat *seat, const char *message)
{
    emit_text(seat_session(seat), LC_EV_LOG, 0, message);
}

static char *dialog_text_flatten(SeatDialogText *text)
{
    strbuf *sb = strbuf_new();
    for (size_t i = 0; i < text->nitems; i++) {
        SeatDialogTextItem *item = &text->items[i];
        switch (item->type) {
          case SDT_PROMPT:
          case SDT_BATCH_ABORT:
          case SDT_TITLE:
            break; /* UI wording belongs to the application */
          case SDT_MORE_INFO_KEY:
            put_fmt(sb, "%s: ", item->text);
            break;
          default:
            put_fmt(sb, "%s\n", item->text);
            break;
        }
    }
    return strbuf_to_str(sb);
}

static SeatPromptResult shim_confirm_ssh_host_key(
    Seat *seat, const char *host, int port, const char *keytype, char *keystr,
    SeatDialogText *text, HelpCtx helpctx,
    void (*callback)(void *ctx, SeatPromptResult result), void *ctx)
{
    lc_session *s = seat_session(seat);
    s->dialog_cb = callback;
    s->dialog_ctx = ctx;
    sfree(s->hk_host);
    sfree(s->hk_type);
    sfree(s->hk_key);
    s->hk_host = dupstr(host);
    s->hk_port = port;
    s->hk_type = dupstr(keytype);
    s->hk_key = dupstr(keystr);

    char *flat = dialog_text_flatten(text);
    lc_event ev;
    memset(&ev, 0, sizeof(ev));
    ev.kind = LC_EV_HOSTKEY;
    ev.user = s->user;
    ev.text = flat;
    ev.str1 = keytype;
    ev.str2 = keystr;
    ev.arg = port;
    emit(&ev);
    sfree(flat);
    return SPR_INCOMPLETE;
}

static SeatPromptResult shim_confirm_weak(
    Seat *seat, SeatDialogText *text,
    void (*callback)(void *ctx, SeatPromptResult result), void *ctx)
{
    lc_session *s = seat_session(seat);
    s->dialog_cb = callback;
    s->dialog_ctx = ctx;
    char *flat = dialog_text_flatten(text);
    emit_text(s, LC_EV_WEAK_CRYPTO, 0, flat);
    sfree(flat);
    return SPR_INCOMPLETE;
}

static const SeatDialogPromptDescriptions *shim_prompt_descriptions(Seat *seat)
{
    static const SeatDialogPromptDescriptions descs = {
        .hk_accept_action = "accept the key",
        .hk_connect_once_action = "connect once",
        .hk_cancel_action = "cancel",
        .hk_cancel_action_Participle = "Cancelling",
        .weak_accept_action = "continue",
        .weak_cancel_action = "cancel",
    };
    return &descs;
}

static bool shim_is_utf8(Seat *seat) { return true; }

static void shim_echoedit_update(Seat *seat, bool echoing, bool editing)
{
    emit_text(seat_session(seat), LC_EV_ECHOEDIT,
              (echoing ? 1 : 0) | (editing ? 2 : 0), NULL);
}

static bool shim_interactive(Seat *seat) { return true; }
static bool shim_verbose(Seat *seat) { return false; }

static StripCtrlChars *shim_stripctrl_new(Seat *seat, BinarySink *bs_out,
                                          SeatInteractionContext sic)
{
    return NULL;
}

static const SeatVtable shim_seat_vt = {
    .output = shim_output,
    .eof = shim_eof,
    .sent = shim_sent,
    .banner = shim_banner,
    .get_userpass_input = shim_get_userpass_input,
    .notify_session_started = shim_notify_session_started,
    .notify_remote_exit = nullseat_notify_remote_exit,
    .notify_remote_disconnect = nullseat_notify_remote_disconnect,
    .connection_fatal = shim_connection_fatal,
    .nonfatal = shim_nonfatal,
    .update_specials_menu = nullseat_update_specials_menu,
    .get_ttymode = nullseat_get_ttymode,
    .set_busy_status = nullseat_set_busy_status,
    .confirm_ssh_host_key = shim_confirm_ssh_host_key,
    .confirm_weak_crypto_primitive = shim_confirm_weak,
    .confirm_weak_cached_hostkey = shim_confirm_weak,
    .prompt_descriptions = shim_prompt_descriptions,
    .is_utf8 = shim_is_utf8,
    .echoedit_update = shim_echoedit_update,
    .get_display = nullseat_get_display,
    .get_windowid = nullseat_get_windowid,
    .get_window_pixel_size = nullseat_get_window_pixel_size,
    .stripctrl_new = shim_stripctrl_new,
    .set_trust_status = nullseat_set_trust_status,
    .can_set_trust_status = nullseat_can_set_trust_status_no,
    .has_mixed_input_stream = nullseat_has_mixed_input_stream_no,
    .verbose = shim_verbose,
    .interactive = shim_interactive,
    .get_cursor_position = nullseat_get_cursor_position,
};

/* ----------------------------------------------------------- LogPolicy -- */

static void shim_lp_eventlog(LogPolicy *lp, const char *event)
{
    lc_session *s = container_of(lp, lc_session, logpolicy);
    emit_text(s, LC_EV_LOG, 0, event);
}

static int shim_lp_askappend(LogPolicy *lp, Filename *filename,
                             void (*callback)(void *ctx, int result),
                             void *ctx)
{
    return 2; /* append; PuTTY logging is off, the application writes logs */
}

static void shim_lp_logging_error(LogPolicy *lp, const char *event)
{
    shim_lp_eventlog(lp, event);
}

static const LogPolicyVtable shim_lp_vt = {
    .eventlog = shim_lp_eventlog,
    .askappend = shim_lp_askappend,
    .logging_error = shim_lp_logging_error,
    .verbose = null_lp_verbose_no,
};

/* ------------------------------------------------------------ Sessions -- */

lc_session *lc_session_open(const lc_setting *settings, size_t n, void *user,
                            char **error_out)
{
    /* Keys prefixed "Jump." describe the jump host's own connection. */
    static const char jump_prefix[] = "Jump.";
    size_t prefix_len = sizeof(jump_prefix) - 1;
    lc_setting *own = snewn(n + 1, lc_setting);
    lc_setting *jump = snewn(n + 1, lc_setting);
    size_t n_own = 0, n_jump = 0;
    const char *proxy_host = NULL;
    for (size_t i = 0; i < n; i++) {
        if (!strncmp(settings[i].key, jump_prefix, prefix_len)) {
            jump[n_jump].key = settings[i].key + prefix_len;
            jump[n_jump++].value = settings[i].value;
        } else {
            if (!strcmp(settings[i].key, "ProxyHost"))
                proxy_host = settings[i].value;
            own[n_own++] = settings[i];
        }
    }
    if (n_jump && proxy_host)
        lc_set_jump_session(proxy_host, lc_settings_from_kv(jump, n_jump));

    settings_r *sr = lc_settings_from_kv(own, n_own);
    sfree(own);
    sfree(jump);
    Conf *conf = conf_new();
    load_open_settings(sr, conf);
    lc_settings_free(sr);

    *error_out = NULL;

    const struct BackendVtable *vt =
        backend_vt_from_proto(conf_get_int(conf, CONF_protocol));
    if (!vt) {
        *error_out = dupstr("protocol not available in this build");
        conf_free(conf);
        return NULL;
    }

    lc_session *s = snew(lc_session);
    memset(s, 0, sizeof(*s));
    s->seat.vt = &shim_seat_vt;
    s->logpolicy.vt = &shim_lp_vt;
    s->user = user;
    s->conf = conf;
    bufchain_init(&s->pending_input);
    s->logctx = log_init(&s->logpolicy, conf);

    /* Link first: PuTTY can call back into the seat during backend_init. */
    s->next = g_sessions;
    g_sessions = s;

    char *realhost = NULL;
    const char *host = conf_get_str(conf, CONF_host);
    if (conf_get_int(conf, CONF_protocol) == PROT_SERIAL)
        host = conf_get_str(conf, CONF_serline);
    char *error = backend_init(vt, &s->seat, &s->backend, s->logctx, conf,
                               host, conf_get_int(conf, CONF_port), &realhost,
                               conf_get_bool(conf, CONF_tcp_nodelay),
                               conf_get_bool(conf, CONF_tcp_keepalives));
    sfree(realhost);
    if (error) {
        /*
         * Every backend sets its handle before it can fail, and a socket that
         * failed synchronously still has a callback queued that would call
         * our seat. Freeing the backend cancels it, so do that before the
         * seat goes away.
         */
        *error_out = error;
        lc_session_close(s);
        return NULL;
    }

    s->ldisc = ldisc_create(conf, NULL, s->backend, &s->seat);
    return s;
}

/*
 * Backends null their socket when the peer closes, and several then dereference
 * it unconditionally. Commands from the application can arrive after that, so
 * everything that reaches a backend goes through this check.
 */
static bool session_live(lc_session *s)
{
    return s->backend && backend_connected(s->backend);
}

static void flush_pending_input(lc_session *s)
{
    if (!session_live(s) || !backend_sendok(s->backend))
        return;
    while (bufchain_size(&s->pending_input) > 0) {
        ptrlen chunk = bufchain_prefix(&s->pending_input);
        backend_send(s->backend, chunk.ptr, chunk.len);
        bufchain_consume(&s->pending_input, chunk.len);
    }
}

void lc_session_send(lc_session *s, const void *data, size_t len)
{
    if (!session_live(s))
        return;
    bufchain_add(&s->pending_input, data, len);
    flush_pending_input(s);
}

void lc_session_size(lc_session *s, int cols, int rows)
{
    if (session_live(s))
        backend_size(s->backend, cols, rows);
}

int lc_session_special(lc_session *s, int code, int arg)
{
    if (!session_live(s))
        return 0;
    SessionSpecialCode sc;
    switch (code) {
      case LC_SPECIAL_BREAK: sc = SS_BRK; break;
      case LC_SPECIAL_EOF: sc = SS_EOF; break;
      case LC_SPECIAL_PING: sc = SS_PING; break;
      case LC_SPECIAL_REKEY: sc = SS_REKEY; break;
      default: return 0;
    }
    backend_special(s->backend, sc, arg);
    return 1;
}

void lc_session_unthrottle(lc_session *s, size_t backlog)
{
    if (session_live(s))
        backend_unthrottle(s->backend, backlog);
}

static void finish_prompt(lc_session *s, SeatPromptResult spr)
{
    prompts_t *p = s->prompt;
    s->prompt_result = spr;
    s->prompt_answered = true;
    if (p->callback)
        queue_toplevel_callback(p->callback, p->callback_ctx);
}

void lc_session_answer_prompt(lc_session *s, const char *const *answers,
                              size_t n)
{
    if (!s->prompt || s->prompt_answered)
        return;
    prompts_t *p = s->prompt;
    for (size_t i = 0; i < p->n_prompts && i < n; i++)
        prompt_set_result(p->prompts[i], answers[i]);
    finish_prompt(s, SPR_OK);
}

void lc_session_cancel_prompt(lc_session *s)
{
    if (!s->prompt || s->prompt_answered)
        return;
    finish_prompt(s, SPR_USER_ABORT);
}

void lc_session_answer_hostkey(lc_session *s, int verdict)
{
    void (*cb)(void *, SeatPromptResult) = s->dialog_cb;
    if (!cb)
        return;
    s->dialog_cb = NULL;
    if (verdict == 2)
        store_host_key(&s->seat, s->hk_host, s->hk_port, s->hk_type,
                       s->hk_key);
    cb(s->dialog_ctx, verdict ? SPR_OK : SPR_USER_ABORT);
}

void lc_session_answer_weak(lc_session *s, int accept)
{
    void (*cb)(void *, SeatPromptResult) = s->dialog_cb;
    if (!cb)
        return;
    s->dialog_cb = NULL;
    cb(s->dialog_ctx, accept ? SPR_OK : SPR_USER_ABORT);
}

void lc_session_close(lc_session *s)
{
    for (lc_session **pp = &g_sessions; *pp; pp = &(*pp)->next) {
        if (*pp == s) {
            *pp = s->next;
            break;
        }
    }
    s->prompt = NULL;
    s->dialog_cb = NULL;
    if (s->ldisc)
        ldisc_free(s->ldisc);
    if (s->backend)
        backend_free(s->backend);
    if (s->logctx)
        log_free(s->logctx);
    bufchain_clear(&s->pending_input);
    conf_free(s->conf);
    sfree(s->fatal_message);
    sfree(s->hk_host);
    sfree(s->hk_type);
    sfree(s->hk_key);
    sfree(s);
}

/* Runs each time round the loop: report sendok and closure transitions. */
static void scan_sessions(void)
{
    lc_session *next;
    for (lc_session *s = g_sessions; s; s = next) {
        next = s->next;
        if (!s->backend)
            continue;
        if (!s->sendok_seen && backend_sendok(s->backend)) {
            s->sendok_seen = true;
            emit_text(s, LC_EV_SENDOK, 0, NULL);
            flush_pending_input(s);
        }
        if (!s->closed_reported && !backend_connected(s->backend)) {
            s->closed_reported = true;
            emit_text(s, LC_EV_CLOSED, backend_exitcode(s->backend),
                      s->fatal_message);
        }
    }
}

/* ---------------------------------------------------------- Event loop -- */

#ifdef _WIN32

static HANDLE g_wake_event;

static int platform_wake_init(void)
{
    g_wake_event = CreateEvent(NULL, FALSE, FALSE, NULL);
    return g_wake_event != NULL ? 0 : -1;
}

void lc_engine_wake(void) { SetEvent(g_wake_event); }

static bool loop_pre(void *ctx, const HANDLE **extra, size_t *n_extra)
{
    scan_sessions();
    *extra = &g_wake_event;
    *n_extra = 1;
    return !g_stop;
}

static bool loop_post(void *ctx, size_t extra_handle_index)
{
    g_cb.on_drain(g_cb.ctx);
    run_toplevel_callbacks();
    scan_sessions();
    return !g_stop;
}

void lc_engine_run(void) { cli_main_loop(loop_pre, loop_post, NULL); }

#else /* unix */

static int g_wake_pipe[2] = {-1, -1};

static int platform_wake_init(void)
{
    if (pipe(g_wake_pipe) < 0)
        return -1;
    for (int i = 0; i < 2; i++) {
        fcntl(g_wake_pipe[i], F_SETFL,
              fcntl(g_wake_pipe[i], F_GETFL) | O_NONBLOCK);
        fcntl(g_wake_pipe[i], F_SETFD, FD_CLOEXEC);
    }
    return 0;
}

void lc_engine_wake(void)
{
    char c = 'x';
    /* A full pipe already means a wakeup is pending. */
    if (write(g_wake_pipe[1], &c, 1) < 0 && errno != EAGAIN)
        return;
}

static bool loop_setup(void *ctx, pollwrapper *pw)
{
    scan_sessions();
    pollwrap_add_fd_rwx(pw, g_wake_pipe[0], SELECT_R);
    return !g_stop;
}

static void loop_check(void *ctx, pollwrapper *pw)
{
    if (pollwrap_check_fd_rwx(pw, g_wake_pipe[0], SELECT_R)) {
        char buf[64];
        while (read(g_wake_pipe[0], buf, sizeof(buf)) > 0)
            ;
    }
    g_cb.on_drain(g_cb.ctx);
}

static bool loop_continue(void *ctx, bool found_any_fd, bool ran_any_callback)
{
    scan_sessions();
    return !g_stop;
}

void lc_engine_run(void)
{
    cli_main_loop(loop_setup, loop_check, loop_continue, NULL);
}

#endif

int lc_engine_init(const lc_callbacks *cb)
{
    g_cb = *cb;
#ifdef _WIN32
    dll_hijacking_protection();
#endif
    enable_dit();
    if (platform_wake_init() < 0)
        return -1;
    sk_init();
#ifndef _WIN32
    uxsel_init();
#endif
    return 0;
}

void lc_engine_stop(void)
{
    g_stop = true;
    lc_engine_wake();
}

void lc_free_string(char *s) { sfree(s); }

const char *lc_putty_version(void) { return sshver; }

/* ------------------------------------------- Things PuTTY expects of us -- */

const unsigned cmdline_tooltype = 0;

void cmdline_error(const char *fmt, ...)
{
    va_list ap;
    va_start(ap, fmt);
    vfprintf(stderr, fmt, ap);
    va_end(ap);
    fputc('\n', stderr);
    abort();
}

char *platform_default_s(const char *name)
{
#ifndef _WIN32
    if (!strcmp(name, "SerialLine"))
        return dupstr("/dev/ttyS0");
#else
    if (!strcmp(name, "SerialLine"))
        return dupstr("COM1");
#endif
    return NULL;
}

bool platform_default_b(const char *name, bool def) { return def; }
int platform_default_i(const char *name, int def) { return def; }
FontSpec *platform_default_fontspec(const char *name)
{
#ifndef _WIN32
    return fontspec_new_default();
#else
    return fontspec_new("", false, 0, 0);
#endif
}
Filename *platform_default_filename(const char *name)
{
    return filename_from_str("");
}

const bool share_can_be_downstream = true;
const bool share_can_be_upstream = true;
