#!/usr/bin/env python3
"""Drives the real linecon window with real input, like a person with no patience.

Runs inside the image from tests/gui/Dockerfile. It reads the screen with OCR, finds
things by their visible text, and saves a screenshot at each step to /out. See
tests/gui/README.md.
"""
import csv
import glob
import io
import os
import subprocess
import sys
import time
import traceback

OUT = "/out"
HOME = "/tmp/h"
CONFIG = f"{HOME}/c/linecon"
IOS = ("192.168.42.101", "admin", "admin")
EOS = ("192.168.42.102", "admin", "admin")
JUNOS = ("192.168.42.103", "admin", "admin@123")
step_no = 0
results = []


def sh(*args, **kw):
    return subprocess.run(args, capture_output=True, text=True, **kw)


def shot(name):
    global step_no
    step_no += 1
    path = f"{OUT}/{step_no:02d}-{name}.png"
    sh("import", "-window", "root", path)
    return path


def words(scale=3):
    """Visible words with screen positions, from OCR."""
    sh("import", "-window", "root", "/tmp/s.png")
    # Upscaled and inverted: light text on dark reads far better this way.
    sh(
        "convert", "/tmp/s.png", "-crop", "1100x700+90+57", "+repage", "-colorspace", "Gray",
        "-resize", f"{scale * 100}%", "-threshold", "45%", "-negate", "/tmp/o.png",
    )
    out = sh("tesseract", "/tmp/o.png", "-", "--psm", "6", "tsv").stdout
    found = []
    for row in csv.DictReader(io.StringIO(out), delimiter="\t", quoting=csv.QUOTE_NONE):
        text = (row.get("text") or "").strip()
        if text:
            x, y = int(row["left"]) / scale + 90, int(row["top"]) / scale + 57
            w, h = int(row["width"]) / scale, int(row["height"]) / scale
            found.append((text, x + w / 2, y + h / 2))
    return found


def screen_text():
    return " ".join(w[0] for w in words())


def wait(text, timeout=25):
    """Waits until `text` is on screen. Any word of several, separated by |."""
    deadline = time.time() + timeout
    options = [o.lower() for o in text.split("|")]
    last = ""
    while time.time() < deadline:
        last = screen_text().lower()
        if any(o in last for o in options):
            return True
        time.sleep(0.5)
    raise AssertionError(f"never saw {text!r}; screen: {last[:400]}")


def gone(text, timeout=10):
    deadline = time.time() + timeout
    while time.time() < deadline:
        if text.lower() not in screen_text().lower():
            return True
        time.sleep(0.4)
    raise AssertionError(f"still showing {text!r}")


def click_xy(x, y, button=1, repeat=1):
    sh("xdotool", "mousemove", str(int(x)), str(int(y)))
    time.sleep(0.15)
    sh("xdotool", "click", "--repeat", str(repeat), "--delay", "90", str(button))
    time.sleep(0.4)


def locate(text, nth=-1, timeout=12):
    deadline = time.time() + timeout
    while time.time() < deadline:
        hits = [w for w in words() if text.lower() in w[0].lower()]
        if hits:
            return hits[nth]
        time.sleep(0.5)
    raise AssertionError(f"could not find {text!r} to click; screen: {screen_text()[:300]}")


def click(text, nth=-1, button=1, repeat=1):
    _, x, y = locate(text, nth)
    click_xy(x, y, button, repeat)


def key(*names):
    sh("xdotool", "key", "--clearmodifiers", *names)
    time.sleep(0.35)


def typed(text):
    sh("xdotool", "type", "--delay", "35", text)
    time.sleep(0.3)


def read_config():
    try:
        return open(f"{CONFIG}/config.toml").read()
    except FileNotFoundError:
        return ""


def start_app():
    env = dict(os.environ, HOME=HOME, XDG_CONFIG_HOME=f"{HOME}/c")
    log = open(f"{OUT}/app.log", "a")
    app = subprocess.Popen(["/bin/linecon"], env=env, stdout=log, stderr=log)
    time.sleep(4)
    return app


def step(name):
    def wrap(fn):
        def run():
            print(f"... {name}", flush=True)
            try:
                fn()
                results.append(("PASS", name, ""))
                print(f"PASS  {name}", flush=True)
            except Exception as e:  # keep going, a person would
                shot("FAIL-" + name.replace(" ", "-")[:40])
                results.append(("FAIL", name, str(e)))
                # Close whatever is open, so one miss does not type into the next step.
                key("Escape")
                time.sleep(0.5)
                print(f"FAIL  {name}: {str(e)[:500]}", flush=True)
                traceback.print_exc(limit=1, file=open(f"{OUT}/trace.log", "a"))
        run.__name__ = name
        STEPS.append(run)
        return run
    return wrap


STEPS = []
QUICK_BOX = (215, 142)
PASSWORD_BOX = (760, 196)
NAME_FIELD = (1020, 198)


@step("first login: quick connect, trust the key, remember the login")
def first_login():
    click_xy(*QUICK_BOX)
    typed(f"{IOS[1]}@{IOS[0]}")
    key("Return")
    wait("Unknown host key")
    shot("hostkey-card")
    click("Trust")
    wait("Remember this")
    shot("password-card")
    click("this")
    time.sleep(0.5)
    shot("remember-ticked")
    # Clicking the box must not leave typing nowhere.
    click_xy(*PASSWORD_BOX)
    typed(IOS[2])
    key("Return")
    wait("passphrase|osxe")


@step("saving the password asks for a passphrase where there is no OS store")
def passphrase():
    if "passphrase" in screen_text().lower():
        shot("passphrase")
        typed("lab-passphrase")
        key("Return")
        gone("Choose a passphrase")
    wait("osxe")
    shot("logged-in")
    time.sleep(1)
    config = read_config()
    assert "Quick sessions" in config, "no Quick sessions folder in the config"
    assert "192.168.42.101" in config, "session not saved"
    assert "has_password = true" in config, config[:600]
    assert IOS[2] + "\n" not in open(f"{CONFIG}/secrets.bin", "rb").read().decode("latin1"), "plaintext"


@step("typing goes to the device without clicking anything")
def typing():
    typed("terminal length 0")
    key("Return")
    typed("show version")
    key("Return")
    wait("Cisco")
    shot("show-version")


@step("Ctrl+C reaches the device, ? and Tab work")
def control_keys():
    typed("?")
    wait("Exec commands")
    key("ctrl+u")
    key("ctrl+c")
    typed("sho")
    key("Tab")
    time.sleep(0.5)
    key("ctrl+u")
    typed("configure terminal")
    key("Return")
    wait("(config)")
    key("ctrl+z")
    # Back at the exec prompt, config commands would not take "show clock".
    typed("show clock")
    key("Return")
    wait("UTC|PDT|EST|PST")
    assert "Invalid" not in screen_text()
    shot("keys")


@step("a second tab connects to Arista with no empty card")
def second_tab():
    click_xy(*QUICK_BOX)
    typed(f"{EOS[1]}@{EOS[0]}")
    key("Return")
    wait("Unknown host key")
    click("Trust")
    wait("password")
    # The field took focus by itself. Typing without clicking it is the test.
    shot("eos-password")
    typed(EOS[2])
    key("Return")
    time.sleep(1)
    assert "Continue" not in screen_text(), "stopped on a card after the password"
    wait("eos")
    time.sleep(1)
    shot("two-tabs")


@step("split, type in the new pane, close it")
def split():
    key("ctrl+shift+d")
    wait("Unknown host key|password|Trust", 15)
    shot("split")
    key("ctrl+shift+w")
    time.sleep(1)
    shot("split-closed")


@step("the finder searches sessions and connects to something new")
def finder():
    key("ctrl+shift+k")
    typed("192.168.42.102")
    # Both the saved session and a new connection to that address.
    wait("Connect to 192")
    shot("finder")
    key("Escape")
    gone("Connect to 192")


@step("the menu row opens About and shows the version")
def menus():
    click("File", 0)
    wait("Preferences")
    shot("menu-file")
    click("About", 0)
    wait("MIT licensed")
    shot("about")
    key("Escape")
    gone("MIT licensed")


def whole_screen_text():
    """For a second window, which can sit outside what words() crops to, and for muted text."""
    sh("import", "-window", "root", "/tmp/whole.png")
    return sh("tesseract", "/tmp/whole.png", "-", "--psm", "11").stdout.lower()


def wait_whole_screen(text, present=True, timeout=15):
    deadline = time.time() + timeout
    while time.time() < deadline:
        if (text.lower() in whole_screen_text()) == present:
            return
        time.sleep(0.4)
    raise AssertionError(f"{text!r} {'never appeared' if present else 'never went away'}")


@step("an open session and a focused field leave the window idle")
def idle_cpu():
    # Redrawing flat out looks fine in every screenshot. Only the CPU shows it.
    click_xy(*QUICK_BOX)
    sh("xdotool", "mousemove", "1250", "780")
    time.sleep(1)
    pid = sh("pgrep", "-x", "linecon").stdout.split()[0]

    def ticks():
        fields = open(f"/proc/{pid}/stat").read().rsplit(")", 1)[1].split()
        return int(fields[11]) + int(fields[12])

    before = ticks()
    time.sleep(5)
    used = (ticks() - before) / os.sysconf("SC_CLK_TCK") / 5
    click_xy(800, 600)
    assert used < 0.5, f"using {used * 100:.0f}% of a core while idle"


@step("the session manager opens ready to search")
def manager_search():
    key("ctrl+shift+o")
    wait("New folder")
    try:
        # Only a search that received the text can say this.
        typed("findme")
        wait_whole_screen("Nothing matches")
        shot("manager-search")
        key(*["BackSpace"] * 6)
        wait_whole_screen("Nothing matches", present=False)
    finally:
        # Left open, it would cover the main window for every later step.
        click("Close", 0)
        gone("New folder")


@step("command palette opens, filters, runs a theme change")
def palette():
    key("ctrl+shift+p")
    # A list entry: the input's hint sits against the text cursor, which misreads.
    wait("Split right")
    shot("palette")
    typed("theme nord")
    time.sleep(0.5)
    key("Return")
    time.sleep(1)
    gone("Split right")
    shot("theme-nord")
    assert 'theme = "Nord"' in read_config()
    key("ctrl+shift+p")
    typed("theme one dark")
    time.sleep(0.5)
    key("Return")
    time.sleep(1)
    shot("theme-back")


@step("the sidebar hides and comes back")
def sidebar():
    click_xy(316, 107)
    gone("Quick sessions")
    shot("sidebar-hidden")
    click_xy(104, 98)
    wait("Quick sessions")
    shot("sidebar-back")


@step("a file browser opens on request, asks here, and lists files")
def files():
    key("ctrl+shift+s")
    wait("Password")
    shot("files-password")
    # The field has focus, and Enter submits, like every other prompt.
    typed(EOS[2])
    key("Return")
    wait("Refresh", 20)
    shot("files")
    key("ctrl+shift+s")
    gone("Refresh")


@step("a device refusing SFTP gets a clear message, not a hang")
def files_refused():
    click_xy(*QUICK_BOX)
    typed(f"{JUNOS[1]}@{JUNOS[0]}")
    key("Return")
    wait("Unknown host key")
    click("Trust")
    wait("password")
    typed(JUNOS[2])
    key("Return")
    wait("junos>", 40)
    key("ctrl+shift+s")
    wait("Password")
    typed(JUNOS[2])
    key("Return")
    wait("Could not open", 20)
    shot("files-refused")
    key("ctrl+shift+s")
    time.sleep(0.5)
    key("ctrl+shift+w")
    time.sleep(1)


@step("session settings open from the row, typing replaces the field")
def settings():
    _, x, y = locate("admin@192.168.42.101", 0)
    click_xy(x, y)
    time.sleep(0.5)
    shot("row-selected")
    click_xy(325, y)
    wait("Protocol")
    shot("settings-panel")
    click_xy(*NAME_FIELD)
    typed("core-router")
    time.sleep(1)
    assert 'name = "core-router"' in read_config(), "typing did not replace the name"
    shot("renamed")
    click_xy(1160, 110)
    gone("Protocol")


@step("a dropped session closes cleanly and the other tab survives")
def close_dead():
    # The second tab is Arista. Its shell ends, then close from the card.
    typed("exit")
    key("Return")
    wait("Disconnected", 15)
    shot("disconnected")
    click("Close")
    time.sleep(1)
    gone("Disconnected")
    shot("after-close")
    # The IOS tab is what is left, and it still works.
    typed("show clock")
    key("Return")
    wait("UTC|PDT|EST|PST")


@step("copy on select, then paste with confirmation for several lines")
def clipboard():
    typed("show clock")
    key("Return")
    time.sleep(1)
    _, x, y = locate("osxe")
    sh("xdotool", "mousemove", str(int(x - 20)), str(int(y)))
    time.sleep(0.2)
    sh("xdotool", "mousedown", "1")
    for dx in (30, 60, 90):
        sh("xdotool", "mousemove", str(int(x - 20 + dx)), str(int(y)))
        time.sleep(0.1)
    sh("xdotool", "mouseup", "1")
    time.sleep(0.6)
    copied = sh("xclip", "-selection", "clipboard", "-o", timeout=5).stdout
    shot("selected")
    assert copied.strip(), "selecting text did not copy it"
    p = subprocess.Popen(["xclip", "-selection", "clipboard", "-i"], stdin=subprocess.PIPE, text=True)
    p.communicate("show clock\nshow users\n")
    time.sleep(0.3)
    key("ctrl+shift+v")
    wait("Paste 2 lines")
    shot("paste-confirm")
    key("Return")
    gone("Paste 2 lines")
    wait("Line|User")
    shot("pasted")


@step("right-click pastes, and asks first for several lines")
def right_click_paste():
    def set_clipboard(text):
        p = subprocess.Popen(["xclip", "-selection", "clipboard", "-i"], stdin=subprocess.PIPE, text=True)
        p.communicate(text)
        time.sleep(0.3)
    set_clipboard("show users")
    click_xy(760, 400, button=3)
    time.sleep(0.8)
    key("Return")
    wait("Line|User")
    set_clipboard("show clock\nshow users\n")
    click_xy(760, 400, button=3)
    wait("Paste 2 lines")
    shot("right-click-paste-confirm")
    key("Escape")
    gone("Paste 2 lines")


@step("search finds text in scrollback")
def search():
    key("ctrl+shift+f")
    wait("Search scrollback")
    typed("Cisco")
    time.sleep(1)
    shot("search")
    # OCR misreads the highlighted match ("Cised"), and the bar holds the word too.
    wait("Cisc")
    # Escape closes the bar, and typing goes back to the device.
    key("Escape")
    time.sleep(0.5)
    shot("search-closed")
    typed("show clock")
    key("Return")
    wait("UTC|PDT|EST|PST")
    assert "Cisco" not in " ".join(w[0] for w in words() if w[1] > 870 and w[2] < 110), "search bar still open"


@step("deleting asks first, and Keep keeps it")
def delete():
    _, x, y = locate("Quick", 0)
    click_xy(x, y, button=3)
    time.sleep(0.5)
    shot("context-menu")
    click("Delete")
    wait("everything")
    shot("delete-confirm")
    click("Keep")
    gone("everything")
    assert "Quick sessions" in read_config()


@step("after a restart the saved login connects with no password prompt")
def restart():
    global APP
    APP.terminate()
    APP.wait(timeout=10)
    time.sleep(1)
    APP = start_app()
    wait("Quick sessions")
    shot("restarted")
    click("core-router", 0, repeat=2)
    wait("Unlock|osxe", 15)
    if "unlock" in screen_text().lower():
        shot("unlock")
        typed("lab-passphrase")
        key("Return")
    wait("osxe", 25)
    text = screen_text().lower()
    assert "continue" not in text, "asked for the password again"
    shot("reconnected")


def main():
    global APP
    os.makedirs(HOME, exist_ok=True)
    subprocess.Popen(["Xvfb", ":99", "-screen", "0", "1280x800x24"], stderr=subprocess.DEVNULL)
    time.sleep(1)
    subprocess.Popen(["openbox"], stderr=subprocess.DEVNULL)
    time.sleep(1)
    APP = start_app()
    shot("start")
    for run in STEPS:
        run()
    APP.terminate()
    print()
    failed = [r for r in results if r[0] == "FAIL"]
    print(f"{len(results) - len(failed)} passed, {len(failed)} failed")
    for _, name, why in failed:
        print(f"  FAIL {name}\n    {why[:400]}")
    sys.exit(1 if failed else 0)


main()
