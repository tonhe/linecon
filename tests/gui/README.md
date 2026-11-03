# Real window checks

These run the actual program in a virtual desktop, with real mouse and keyboard input, an X server, a window
manager and software OpenGL, against the device lab. They catch what headless tests cannot: drawing, focus, the
clipboard and things laid out where nobody can see them.

    # Bring up the lab first: see tests/lab/README.md.
    mkdir -p /tmp/gui-shots
    tests/gui/run.sh target/debug/linecon /tmp/gui-shots

Each step prints PASS or FAIL and saves screenshots. Look at the screenshots too: passing means the flow works,
not that it looks right. Text is read with OCR, which misreads some characters, so checks match loosely.

Run these before tagging a release. They cover Linux only. macOS and Windows window behaviour still needs a person.
