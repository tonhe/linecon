# Session bundle format

A session bundle is a TOML file that moves sessions between installs. The exporter names it
`linecon-sessions.toml`.

```toml
format = "linecon-sessions-1"

# Settings every session in this file inherits, unless overridden further down.
[settings]
legacy_algorithms = true

# Present only when exported with logins. Never holds passwords.
[[identities]]
id = 3
name = "netops"
username = "netops"
key_file = "/keys/netops.ppk"
use_agent = true

[[nodes]]
kind = "folder"
id = 1
name = "Core switches"

[nodes.settings]
identity = 3
theme = "Nord"

[[nodes.children]]
kind = "session"
id = 2
name = "core-sw1"

[nodes.children.target]
protocol = "ssh"       # ssh, telnet, serial or raw
host = "10.20.0.2"
port = 22              # optional
# line = "/dev/ttyUSB0"  for serial
```

- `id` values only link things within the file. Importing assigns new ones.
- A folder's `settings` and `snippets` apply to everything beneath it. Anything left out is inherited.
- A setting named in `settings` follows the same names as `config.toml`: `identity`, `theme`, `font_size`,
  `scrollback_lines`, `copy_on_select`, `jump_host`, `port_forwards`, `logging`, `legacy_algorithms`,
  `cipher_order`, `kex_order`, `host_key_order`, `keepalive_secs`, `auto_reconnect`, `serial_baud`,
  `serial_data_bits`, `serial_stop_half_bits`, `serial_parity` and `serial_flow`.

The `format` value changes if a change would break older readers. A reader refuses a version it does not know.

Passwords are never written. Log folders are left out, because they only mean something on one machine.
