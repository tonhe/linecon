# Stands in for current gear like IOS XE: an RSA host key and nothing else. One
# port signs with SHA-2, which must work by default. The other only offers SHA-1
# ssh-rsa, which must stay behind the legacy toggle.
FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends openssh-server \
 && mkdir -p /run/sshd \
 && useradd -m -s /bin/sh lab \
 && echo 'lab:lab-password' | chpasswd \
 && rm -f /etc/ssh/ssh_host_* \
 && ssh-keygen -q -t rsa -b 3072 -N '' -f /etc/ssh/rsa
COPY sshd-*.conf /etc/ssh/
COPY start.sh /start.sh
EXPOSE 2301 2302
CMD ["/bin/sh", "/start.sh"]
