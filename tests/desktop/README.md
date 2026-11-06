# Desktop key checks

CI presses real keys in the real window on Windows and macOS, and checks the exact bytes a
device receives. The device is `fake_device.py`, a TCP server reached as a raw session.

    python tests/desktop/windows_keys.py target/release/linecon.exe keys-screen.png   # pip install pywinauto
    python3 tests/desktop/mac_keys.py target/release/linecon keys-screen.png          # needs Accessibility access

Each script runs a portable copy of the binary, so it starts with no settings and leaves none
behind. On Windows the quick connect box is found by its screen reader name.

Known gap: on Windows, Ctrl+V with an empty clipboard sends nothing. egui-winit turns Ctrl+V
into a paste and drops the key when there is no text to paste.
