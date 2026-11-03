# A real desktop session for driving the real window: X server, window manager,
# software OpenGL, input injection and screenshots. Nothing here ships.
FROM ubuntu:24.04
RUN apt-get update && apt-get install -y --no-install-recommends \
    xvfb openbox xdotool imagemagick x11-utils xclip python3 tesseract-ocr \
    libgl1 libegl1 libgl1-mesa-dri libglx-mesa0 libxkbcommon-x11-0 libxcursor1 libxi6 libxrandr2 \
    fonts-dejavu-core
ENV DISPLAY=:99 LIBGL_ALWAYS_SOFTWARE=1 XDG_RUNTIME_DIR=/tmp
