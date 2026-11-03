#!/bin/sh
for conf in /etc/ssh/sshd-*.conf; do
    /usr/sbin/sshd -f "$conf" -e || exit 1
done
exec tail -f /dev/null
