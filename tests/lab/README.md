# Device lab checks

CI cannot run these because the device images are licensed. Run them on a machine with netlab, containerlab and
the images in `topology.yml`, before tagging a release.

    mkdir -p ~/labs/linecon0-matrix && cp tests/lab/topology.yml ~/labs/linecon0-matrix/
    (cd ~/labs/linecon0-matrix && netlab up)
    cargo build -p linecon
    B=$PWD/target/debug/linecon

    # Default settings must log in to current gear with no flags.
    tests/lab/check_devices.py $B iosxe 192.168.42.101 admin admin "show version" "Cisco IOS"
    tests/lab/check_devices.py $B eos 192.168.42.102 admin admin "show version" "Arista"
    tests/lab/check_devices.py $B junos 192.168.42.103 admin 'admin@123' "show version" "Junos"

    # The keys people use all day.
    tests/lab/check_ios_keys.py $B 192.168.42.101 admin admin

    # The same devices through the window's code path.
    LINECON_LAB='iosxe,192.168.42.101,admin,admin;eos,192.168.42.102,admin,admin;junos,192.168.42.103,admin,admin@123' \
      cargo test -p linecon real_devices -- --ignored --nocapture

    (cd ~/labs/linecon0-matrix && netlab down)

The addresses follow `defaults.multilab.id` in `topology.yml`. The credentials are netlab's lab defaults.
