# OpenSSH 6.7 is the newest packaged server that still does SSH-1, arcfour,
# ssh-dss, group1-sha1 and SHA-1-only ssh-rsa. Current distros removed all of
# them, so this image exists to stand in for old network hardware.
FROM debian/eol:jessie
RUN printf 'deb [trusted=yes] http://archive.debian.org/debian jessie main\n' > /etc/apt/sources.list \
 && apt-get -o Acquire::Check-Valid-Until=false update \
 && apt-get install -y --no-install-recommends --force-yes openssh-server \
 && mkdir -p /var/run/sshd \
 && useradd -m -s /bin/sh lab \
 && echo 'lab:lab-password' | chpasswd \
 && rm -f /etc/ssh/ssh_host_* \
 && ssh-keygen -q -t rsa -b 1024 -N '' -f /etc/ssh/rsa1024 \
 && ssh-keygen -q -t dsa -N '' -f /etc/ssh/dsa1024 \
 && ssh-keygen -q -t rsa1 -b 1024 -N '' -f /etc/ssh/rsa1
COPY sshd-*.conf /etc/ssh/
COPY start.sh /start.sh
EXPOSE 2201 2202 2203 2204
CMD ["/bin/sh", "/start.sh"]
