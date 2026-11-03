//! Drives the real UI headlessly. Structural checks run everywhere. Set
//! LINECON_SCREENSHOT_DIR to also render PNGs, which needs a GPU or a software
//! Vulkan driver, and is how the README images are produced.

use std::path::PathBuf;

use egui_kittest::kittest::Queryable;
use egui_kittest::Harness;
use linecon_config::{ConfigLocation, Id, Mode, Protocol, Target};
use linecon_engine::algorithms::AlgorithmPolicy;
use linecon_engine::negotiation::{NegotiationFailure, ServerOffer};
use linecon_engine::{HostKeyPrompt, Prompt, PromptField};

use crate::app::App;
use crate::pane::Pending;

const SHOW_VERSION: &[u8] = b"\x1b[1;32mcore-sw1#\x1b[0m show version\r\n\
Cisco IOS Software, C3750E Software (C3750E-UNIVERSALK9-M), Version 15.2(4)E10\r\n\
Copyright (c) 1986-2020 by Cisco Systems, Inc.\r\n\r\n\
core-sw1 uptime is 3 years, 12 weeks, 2 days, 4 hours, 11 minutes\r\n\
System image file is \"flash:c3750e-universalk9-mz.152-4.E10.bin\"\r\n\r\n\
\x1b[1;32mcore-sw1#\x1b[0m show ip interface brief\r\n\
Interface              IP-Address      OK? Method Status                Protocol\r\n\
Vlan1                  10.20.0.2       YES NVRAM  \x1b[32mup\x1b[0m                    \x1b[32mup\x1b[0m\r\n\
GigabitEthernet1/0/1   unassigned      YES unset  \x1b[32mup\x1b[0m                    \x1b[32mup\x1b[0m\r\n\
GigabitEthernet1/0/2   unassigned      YES unset  \x1b[31mdown\x1b[0m                  \x1b[31mdown\x1b[0m\r\n\
GigabitEthernet1/0/3   unassigned      YES unset  \x1b[33madministratively down\x1b[0m \x1b[31mdown\x1b[0m\r\n\
Port-channel1          unassigned      YES unset  \x1b[32mup\x1b[0m                    \x1b[32mup\x1b[0m\r\n\r\n\
\x1b[1;32mcore-sw1#\x1b[0m ";

const TOP: &[u8] = b"\x1b[7m  PID USER      PR  NI    VIRT    RES  %CPU %MEM COMMAND       \x1b[0m\r\n\
  812 root      20   0  114.2m   9.1m   2.3  0.2 \x1b[1mbgpd\x1b[0m\r\n\
  640 root      20   0   88.4m   7.7m   1.1  0.2 \x1b[1mospfd\x1b[0m\r\n\
\r\n\x1b[38;2;255;150;50mtrue color\x1b[0m \x1b[38;5;45m256 color\x1b[0m \x1b[4munderline\x1b[0m\r\n";

fn location() -> ConfigLocation {
    let dir = std::env::temp_dir().join(format!("linecon-ui-{}", std::process::id()));
    ConfigLocation {
        mode: Mode::Portable,
        dir,
    }
}

fn target(host: &str) -> Target {
    Target {
        host: host.into(),
        ..Default::default()
    }
}

struct Sample {
    app: App,
    core_sw: Id,
}

fn sample() -> Sample {
    let mut app = App::new(location());
    let c = &mut app.config;
    let dc = c.add_folder(None, "Datacenter").unwrap();
    c.overrides_mut(dc).unwrap().theme = Some("Tokyo Night".into());
    let core = c.add_folder(Some(dc), "Core switches").unwrap();
    c.overrides_mut(core).unwrap().legacy_algorithms = Some(true);
    let core_sw = c
        .add_session(Some(core), "core-sw1", target("10.20.0.2"))
        .unwrap();
    c.add_session(Some(core), "core-sw2", target("10.20.0.3"))
        .unwrap();
    let edge = c.add_folder(Some(dc), "Edge").unwrap();
    c.add_session(Some(edge), "edge-rtr1", target("10.20.1.1"))
        .unwrap();
    let branch = c.add_folder(None, "Branch office").unwrap();
    c.add_session(Some(branch), "br-sw1", target("172.16.4.2"))
        .unwrap();
    c.add_session(
        Some(branch),
        "console-server",
        Target {
            protocol: Protocol::Telnet,
            host: "172.16.4.9".into(),
            port: Some(2003),
            line: String::new(),
        },
    )
    .unwrap();
    c.add_session(
        None,
        "Bench console",
        Target {
            protocol: Protocol::Serial,
            line: "/dev/ttyUSB0".into(),
            ..Default::default()
        },
    )
    .unwrap();
    Sample { app, core_sw }
}

fn harness(app: App) -> Harness<'static, App> {
    Harness::builder()
        .with_size(egui::vec2(1180.0, 720.0))
        .with_pixels_per_point(2.0)
        .with_max_steps(8)
        .build_ui_state(|ui, app: &mut App| app.show(ui), app)
}

fn screenshot(harness: &mut Harness<'_, App>, name: &str) {
    let Some(dir) = std::env::var_os("LINECON_SCREENSHOT_DIR") else {
        return;
    };
    let dir = PathBuf::from(dir);
    std::fs::create_dir_all(&dir).unwrap();
    let image = harness.render().expect("render");
    image.save(dir.join(format!("{name}.png"))).unwrap();
}

#[test]
fn default_view_is_terminal_and_sidebar_only() {
    let mut s = sample();
    s.app
        .add_demo_tab("core-sw1", Some(s.core_sw), SHOW_VERSION);
    let mut h = harness(s.app);
    h.run_steps(3);
    // The tree is there, and nothing else asks for attention.
    assert!(h.query_by_label("Global settings").is_none());
    screenshot(&mut h, "main");
}

#[test]
fn empty_state() {
    let mut h = harness(App::new(location()));
    h.run_steps(3);
    screenshot(&mut h, "empty");
}

#[test]
fn inspector_shows_where_values_come_from() {
    let mut s = sample();
    s.app
        .add_demo_tab("core-sw1", Some(s.core_sw), SHOW_VERSION);
    s.app.inspect(s.core_sw);
    let mut h = harness(s.app);
    h.run_steps(3);
    assert!(h.query_by_label("from Core switches").is_some());
    assert!(h.query_by_label("from Datacenter").is_some());
    assert!(h.query_all_by_label("default").count() > 1);
    screenshot(&mut h, "inspector");
}

#[test]
fn clearing_an_override_returns_to_inheriting() {
    let mut s = sample();
    let id = s.core_sw;
    s.app.config.overrides_mut(id).unwrap().theme = Some("Nord".into());
    s.app.inspect(id);
    let mut h = harness(s.app);
    h.run_steps(3);
    assert!(h.query_by_label("set here").is_some());
    h.get_all_by_label("×").nth(1).unwrap().click();
    h.run_steps(3);
    let overrides = h.state_mut().config.overrides_mut(id).unwrap().clone();
    assert_eq!(overrides.theme, None);
    assert!(h.query_by_label("set here").is_none());
}

#[test]
fn split_panes_and_tabs() {
    let mut s = sample();
    s.app
        .add_demo_tab("core-sw1", Some(s.core_sw), SHOW_VERSION);
    s.app.split_demo(TOP);
    s.app.add_demo_tab("edge-rtr1", None, TOP);
    assert_eq!(s.app.tab_count(), 2);
    assert_eq!(s.app.pane_count(), 3);
    let mut h = harness(s.app);
    h.run_steps(3);
    screenshot(&mut h, "tabs");
    h.state_mut().close_focused();
    assert_eq!(h.state().tab_count(), 1);
    h.run_steps(3);
    screenshot(&mut h, "split");
}

#[test]
fn palette_filters() {
    let mut s = sample();
    s.app
        .add_demo_tab("core-sw1", Some(s.core_sw), SHOW_VERSION);
    s.app.open_palette("theme");
    let mut h = harness(s.app);
    h.run_steps(3);
    screenshot(&mut h, "palette");
}

fn failure() -> NegotiationFailure {
    let offer = ServerOffer {
        banner: "SSH-2.0-Cisco-1.25".into(),
        kex: vec!["diffie-hellman-group1-sha1".into()],
        host_keys: vec!["ssh-rsa".into()],
        ciphers_c2s: vec!["aes128-cbc".into(), "3des-cbc".into()],
        ciphers_s2c: vec!["aes128-cbc".into(), "3des-cbc".into()],
        macs_c2s: vec!["hmac-sha1".into(), "hmac-sha1-96".into()],
        macs_s2c: vec!["hmac-sha1".into()],
    };
    NegotiationFailure::new(
        "The first key exchange algorithm supported by the server is diffie-hellman-group1-sha1, \
         which is below the configured warning threshold.",
        &AlgorithmPolicy::modern(),
        Some(offer),
    )
}

#[test]
fn negotiation_failure_card_shows_both_sides() {
    let mut s = sample();
    s.app.add_demo_tab("core-sw1", Some(s.core_sw), b"");
    s.app.demo_failure(failure());
    let mut h = harness(s.app);
    h.run_steps(3);
    assert!(h.query_by_label("Could not agree on algorithms").is_some());
    assert!(h
        .query_by_label("Allow legacy algorithms and retry")
        .is_some());
    screenshot(&mut h, "failure");
}

#[test]
fn prompts_and_host_keys_appear_as_cards() {
    let mut s = sample();
    s.app.add_demo_tab("core-sw1", Some(s.core_sw), b"");
    s.app.demo_pending(Pending::HostKey(HostKeyPrompt {
        key_type: "ssh-rsa".into(),
        key: "0x10001,0xabc".into(),
        port: 22,
        description: "The host key is not cached for this server:\n  10.20.0.2 (port 22)\n\
             You have no guarantee that the server is the computer you think it is.\n\
             The server's rsa2 key fingerprint is:\n  ssh-rsa 1024 SHA256:Qm1t9V0mB2Xk7eQe1n3Y"
            .into(),
    }));
    let mut h = harness(s.app);
    h.run_steps(3);
    assert!(h.query_by_label("Trust and remember").is_some());
    screenshot(&mut h, "hostkey");
    h.state_mut().demo_pending(Pending::Prompt(Prompt {
        name: Some("SSH password".into()),
        instruction: None,
        from_server: false,
        fields: vec![PromptField {
            text: "admin@10.20.0.2's password: ".into(),
            echo: false,
        }],
    }));
    h.run_steps(3);
    assert!(h.query_by_label("Continue").is_some());
    screenshot(&mut h, "password");
}

#[test]
fn every_builtin_theme_renders() {
    for theme in crate::themes::builtin() {
        let mut s = sample();
        s.app.config.settings.theme = Some(theme.name.clone());
        s.app.config.remove(1);
        s.app.add_demo_tab("core-sw1", None, SHOW_VERSION);
        let mut h = harness(s.app);
        h.run_steps(2);
        screenshot(
            &mut h,
            &format!("theme-{}", theme.name.replace(' ', "-").to_lowercase()),
        );
    }
}

/// Keys pressed in the window reach a real connection, and its output reaches
/// the grid. This goes through the whole stack, PuTTY included.
#[test]
fn typing_reaches_the_peer_and_output_reaches_the_screen() {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.write_all(b"ready\r\n").unwrap();
        let mut buf = [0u8; 256];
        while let Ok(n) = stream.read(&mut buf) {
            if n == 0 {
                break;
            }
            tx.send(buf[..n].to_vec()).unwrap();
        }
    });

    let mut app = App::new(location());
    app.quick_open(&format!("raw://127.0.0.1:{port}"));
    let mut h = harness(app);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !h.state().focused_screen_contains("ready") {
        assert!(
            std::time::Instant::now() < deadline,
            "output never reached the grid"
        );
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    h.step();
    h.key_press_modifiers(egui::Modifiers::CTRL, egui::Key::Z);
    h.key_press(egui::Key::ArrowUp);
    h.step();
    let mut seen = Vec::new();
    while seen.len() < 4 {
        seen.extend(
            rx.recv_timeout(std::time::Duration::from_secs(20))
                .expect("peer got input"),
        );
    }
    assert_eq!(seen, [0x1a, 0x1b, b'[', b'A']);
}
