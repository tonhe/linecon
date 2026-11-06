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
    // One folder per app: tests run in parallel, and a shared config file would
    // let one test load the tree another just saved.
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("linecon-ui-{}-{n}", std::process::id()));
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
    assert!(h.query_by_label("core-sw2").is_some());
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
    h.get_by_label("Terminal").click();
    h.run_steps(3);
    assert!(h.query_by_label("from Datacenter").is_some());
    assert!(h.query_all_by_label("default").count() > 1);
    screenshot(&mut h, "inspector");
    h.get_by_label("SSH").click();
    h.run_steps(3);
    assert!(h.query_by_label("from Core switches").is_some());
}

#[test]
fn clearing_an_override_returns_to_inheriting() {
    let mut s = sample();
    let id = s.core_sw;
    s.app.config.overrides_mut(id).unwrap().theme = Some("Nord".into());
    s.app.inspect(id);
    let mut h = harness(s.app);
    h.run_steps(3);
    h.get_by_label("Terminal").click();
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
        known_types: Vec::new(),
        changed: false,
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

/// A saved identity logs in to the legacy server with nothing asked of the user
/// but the host key, and a wrong one falls back to asking instead of retrying.
#[test]
fn saved_password_logs_in_without_asking() {
    use crate::pane::{Pane, PaneSpec, Status};
    use linecon_engine::HostKeyVerdict;
    use std::process::Command;
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use zeroize::Zeroizing;

    let docker = |args: &[&str]| -> Option<String> {
        let out = Command::new("docker").args(args).output().ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let context = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/docker/legacy-sshd"
    );
    let name = format!("linecon-identity-{}", std::process::id());
    let started = docker(&["build", "-q", "-t", "linecon-legacy-sshd", context]).and_then(|_| {
        docker(&[
            "run",
            "-d",
            "--rm",
            "--name",
            &name,
            "-P",
            "linecon-legacy-sshd",
        ])
    });
    if started.is_none() {
        assert!(
            std::env::var_os("LINECON_REQUIRE_DOCKER").is_none(),
            "Docker is required here"
        );
        eprintln!("SKIPPED: no container runtime");
        return;
    }
    struct Stop<'a>(&'a str);
    impl Drop for Stop<'_> {
        fn drop(&mut self) {
            let _ = Command::new("docker").args(["rm", "-f", self.0]).output();
        }
    }
    let _stop = Stop(&name);
    let port: u16 = docker(&["port", &name, "2201"])
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .rsplit(':')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    std::thread::sleep(Duration::from_secs(1));

    let mut config = linecon_config::Config::default();
    let who = config.add_identity("lab", "lab");
    config.identity_mut(who).unwrap().has_password = true;
    config.settings.identity = Some(who);
    config.settings.legacy_algorithms = Some(true);
    let target = Target {
        host: "127.0.0.1".into(),
        port: Some(port),
        ..Default::default()
    };
    let resolved = config.resolve_root();
    let settings = resolved.engine_settings_for(&config, &target, "");

    let run = |password: &'static str| {
        let spec = PaneSpec {
            name: "lab".into(),
            session_id: None,
            settings: settings.clone(),
            policy: resolved.algorithm_policy(),
            log_dir: None,
            scrollback: 1000,
            copy_on_select: true,
            auto_reconnect: false,
            accept_weak: true,
            trust_new_host_keys: false,
            password: Some(Arc::new(move || Some(Zeroizing::new(password.to_string())))),
        };
        Pane::new(spec, Arc::new(|| {}))
    };

    // Right password: only the host key is asked.
    let pane = run("lab-password");
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        assert!(Instant::now() < deadline, "never went live");
        let pending = pane.shared.pending.lock().unwrap().clone();
        match pending {
            Some(Pending::HostKey(_)) => pane.answer_hostkey(HostKeyVerdict::AcceptOnce),
            Some(Pending::Prompt(p)) => panic!("asked the user anyway: {p:?}"),
            None => {}
        }
        if pane.status() == Status::Live {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(pane);

    // Wrong password: tried once, then the user is asked.
    let pane = run("wrong-password");
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        assert!(Instant::now() < deadline, "the user was never asked");
        let pending = pane.shared.pending.lock().unwrap().clone();
        match pending {
            Some(Pending::HostKey(_)) => pane.answer_hostkey(HostKeyVerdict::AcceptOnce),
            Some(Pending::Prompt(p)) => {
                assert!(p.fields[0].text.contains("password"), "{p:?}");
                break;
            }
            None => {}
        }
        assert_ne!(pane.status(), Status::Live);
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn snippets_are_offered_only_within_their_scope() {
    let mut s = sample();
    let folder = s.app.config.ancestors(s.core_sw).unwrap()[1].id;
    if let Some(linecon_config::Node::Folder(f)) = s.app.config.find_mut(folder) {
        f.snippets.push(linecon_config::Snippet {
            name: "core health".into(),
            text: "show env\n".into(),
        });
    }
    s.app.add_demo_tab("elsewhere", None, b"");
    s.app.open_palette("run:");
    let mut h = harness(s.app);
    h.run_steps(2);
    assert!(
        h.query_by_label("Run: core health").is_none(),
        "offered outside its folder"
    );
    let id = s_core(&h);
    h.state_mut().add_demo_tab("core-sw1", Some(id), b"");
    h.state_mut().open_palette("run:");
    h.run_steps(2);
    assert!(
        h.query_by_label("Run: core health").is_some(),
        "not offered inside its folder"
    );
    screenshot(&mut h, "snippet");
}

fn s_core(h: &Harness<'_, App>) -> Id {
    fn walk(nodes: &[linecon_config::Node]) -> Option<Id> {
        nodes.iter().find_map(|n| match n {
            linecon_config::Node::Session(s) if s.name == "core-sw1" => Some(s.id),
            linecon_config::Node::Folder(f) => walk(&f.children),
            _ => None,
        })
    }
    walk(&h.state().config.nodes).unwrap()
}

#[test]
fn dropping_session_files_imports_them() {
    let dir = std::env::temp_dir().join(format!("linecon-drop-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("config");
    std::fs::write(&file, "Host sw7\n  HostName 10.7.7.7\n  User ops\n").unwrap();
    let mut app = App::new(location());
    app.import_for_test(&file);
    let imported = app
        .config
        .nodes
        .iter()
        .find(|n| n.name() == "Imported from SSH config");
    assert!(imported.is_some());
    assert_eq!(app.config.identities[0].username, "ops");
    std::fs::remove_dir_all(dir).unwrap();
}

/// A console server dropping the connection comes back by itself.
#[test]
fn a_dropped_connection_reconnects_when_asked_to() {
    use crate::pane::{Pane, PaneSpec, Status};
    use linecon_engine::algorithms::AlgorithmPolicy;
    use linecon_engine::settings::Settings;
    use std::io::Write;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        // First connection is dropped straight away, the second stays up.
        let (mut first, _) = listener.accept().unwrap();
        first.write_all(b"first\r\n").unwrap();
        std::thread::sleep(Duration::from_millis(200));
        drop(first);
        let (mut second, _) = listener.accept().unwrap();
        second.write_all(b"second\r\n").unwrap();
        std::thread::sleep(Duration::from_secs(30));
    });
    let spec = PaneSpec {
        name: "console".into(),
        session_id: None,
        settings: Settings::raw("127.0.0.1", port),
        policy: AlgorithmPolicy::modern(),
        log_dir: None,
        scrollback: 1000,
        copy_on_select: true,
        auto_reconnect: true,
        accept_weak: false,
        trust_new_host_keys: false,
        password: None,
    };
    let mut pane = Pane::new(spec, Arc::new(|| {}));
    let deadline = Instant::now() + Duration::from_secs(20);
    let sees = |pane: &Pane, text: &str| {
        pane.shared
            .core
            .lock()
            .unwrap()
            .visible_lines()
            .iter()
            .any(|l| l.contains(text))
    };
    while !sees(&pane, "second") {
        assert!(
            Instant::now() < deadline,
            "never reconnected: {:?}",
            pane.status()
        );
        pane.tick_reconnect();
        std::thread::sleep(Duration::from_millis(50));
    }
    // What came before the drop is still on screen.
    assert!(sees(&pane, "first"));
    assert_eq!(pane.status(), Status::Live);
}

/// The legacy toggle alone gets a user onto an SSH-1-only device.
#[test]
fn legacy_toggle_reaches_an_ssh1_only_device() {
    use crate::pane::{Pane, PaneSpec, Status};
    use linecon_engine::HostKeyVerdict;
    use std::process::Command;
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use zeroize::Zeroizing;

    let docker = |args: &[&str]| -> Option<String> {
        let out = Command::new("docker").args(args).output().ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let context = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/docker/legacy-sshd"
    );
    let name = format!("linecon-ssh1-{}", std::process::id());
    let started = docker(&["build", "-q", "-t", "linecon-legacy-sshd", context]).and_then(|_| {
        docker(&[
            "run",
            "-d",
            "--rm",
            "--name",
            &name,
            "-P",
            "linecon-legacy-sshd",
        ])
    });
    if started.is_none() {
        assert!(
            std::env::var_os("LINECON_REQUIRE_DOCKER").is_none(),
            "Docker is required here"
        );
        eprintln!("SKIPPED: no container runtime");
        return;
    }
    struct Stop<'a>(&'a str);
    impl Drop for Stop<'_> {
        fn drop(&mut self) {
            let _ = Command::new("docker").args(["rm", "-f", self.0]).output();
        }
    }
    let _stop = Stop(&name);
    let port: u16 = docker(&["port", &name, "2203"])
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .rsplit(':')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    std::thread::sleep(Duration::from_secs(1));

    let mut config = linecon_config::Config::default();
    config.settings.legacy_algorithms = Some(true);
    let resolved = config.resolve_root();
    let target = Target {
        host: "127.0.0.1".into(),
        port: Some(port),
        ..Default::default()
    };
    let spec = PaneSpec {
        name: "ancient".into(),
        session_id: None,
        settings: resolved.engine_settings(&target, "lab"),
        policy: resolved.algorithm_policy(),
        log_dir: None,
        scrollback: 1000,
        copy_on_select: true,
        auto_reconnect: false,
        accept_weak: true,
        trust_new_host_keys: false,
        password: Some(Arc::new(|| {
            Some(Zeroizing::new("lab-password".to_string()))
        })),
    };
    let mut pane = Pane::new(spec, Arc::new(|| {}));
    let deadline = Instant::now() + Duration::from_secs(30);
    while pane.status() != Status::Live {
        assert!(
            Instant::now() < deadline,
            "never connected: {:?}",
            pane.status()
        );
        let pending = pane.shared.pending.lock().unwrap().clone();
        if let Some(Pending::HostKey(_)) = pending {
            pane.answer_hostkey(HostKeyVerdict::AcceptOnce);
        }
        pane.tick_reconnect();
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        pane.spec.policy.version,
        linecon_engine::algorithms::SshVersion::V1
    );
}

#[test]
fn search_finds_scrollback_and_steps_between_matches() {
    use crate::pane::{Pane, PaneSpec};
    use crate::terminal_view::{search_step, Search};
    use alacritty_terminal::grid::Dimensions;
    use std::sync::Arc;

    let spec = PaneSpec {
        name: "s".into(),
        session_id: None,
        settings: linecon_engine::settings::Settings::raw("demo", 1),
        policy: AlgorithmPolicy::modern(),
        log_dir: None,
        scrollback: 10_000,
        copy_on_select: true,
        auto_reconnect: false,
        accept_weak: false,
        trust_new_host_keys: false,
        password: None,
    };
    let pane = Pane::offline(spec, Arc::new(|| {}));
    {
        let mut core = pane.shared.core.lock().unwrap();
        for i in 0..500 {
            let line = if i == 20 || i == 300 {
                format!("interface Gi{i}\r\n")
            } else {
                format!("line {i}\r\n")
            };
            core.feed(line.as_bytes());
        }
    }
    let line_of = |search: &Search| search.current.expect("a match").0.line.0;
    let shown = |search: &Search| {
        let core = pane.shared.core.lock().unwrap();
        let offset = core.term.grid().display_offset() as i32;
        let row = line_of(search) + offset;
        row >= 0 && row < core.term.screen_lines() as i32
    };

    let mut search = Search {
        query: "interface".into(),
        ..Default::default()
    };
    // Backwards from the live screen reaches the newer match first.
    search_step(&pane, &mut search, false);
    let newer = line_of(&search);
    assert!(shown(&search), "the match was not scrolled into view");
    search_step(&pane, &mut search, false);
    let older = line_of(&search);
    assert!(
        older < newer,
        "stepping back did not move: {older} vs {newer}"
    );
    assert!(shown(&search));
    search_step(&pane, &mut search, true);
    assert_eq!(line_of(&search), newer, "stepping forward did not return");

    let mut search = Search {
        query: "no-such-text".into(),
        ..Default::default()
    };
    search_step(&pane, &mut search, false);
    assert!(search.no_match && search.current.is_none());
    let mut search = Search {
        query: "(unclosed".into(),
        ..Default::default()
    };
    search_step(&pane, &mut search, false);
    assert!(search.no_match, "a bad pattern must not panic or match");
}

#[test]
fn file_browser_lists_entries_only_when_opened() {
    use crate::sftp_pane::SftpPane;
    use linecon_engine::sftp::{Attrs, DirEntry};
    let entry = |name: &str, permissions: u32, size: u64| DirEntry {
        name: name.into(),
        attrs: Attrs {
            size: Some(size),
            permissions: Some(permissions),
            modified: None,
        },
    };
    let mut s = sample();
    s.app
        .add_demo_tab("core-sw1", Some(s.core_sw), SHOW_VERSION);
    let mut h = harness(s.app);
    h.run_steps(2);
    assert!(
        h.query_by_label("Files").is_none(),
        "the browser must not show by default"
    );
    h.state_mut().demo_files(SftpPane::demo(
        "/home/netops",
        vec![
            entry("backups", 0o040755, 4096),
            entry("running-config.txt", 0o100644, 18_204),
            entry("c3750e-universalk9-mz.152-4.E10.bin", 0o100644, 26_214_400),
        ],
    ));
    h.run_steps(3);
    assert!(h.query_by_label("Files").is_some());
    assert!(h.query_by_label("running-config.txt").is_some());
    assert!(h.query_by_label("/home/netops").is_some());
    screenshot(&mut h, "files");
}

/// The worker end to end: connect on the session's settings, then move a file
/// each way using the same calls the panel makes.
#[test]
fn file_browser_moves_files_against_the_legacy_server() {
    use crate::sftp_pane::{Question, SftpPane};
    use linecon_engine::HostKeyVerdict;
    use std::process::Command;
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use zeroize::Zeroizing;

    let docker = |args: &[&str]| -> Option<String> {
        let out = Command::new("docker").args(args).output().ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let context = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/docker/legacy-sshd"
    );
    let name = format!("linecon-files-{}", std::process::id());
    let started = docker(&["build", "-q", "-t", "linecon-legacy-sshd", context]).and_then(|_| {
        docker(&[
            "run",
            "-d",
            "--rm",
            "--name",
            &name,
            "-P",
            "linecon-legacy-sshd",
        ])
    });
    if started.is_none() {
        assert!(
            std::env::var_os("LINECON_REQUIRE_DOCKER").is_none(),
            "Docker is required here"
        );
        eprintln!("SKIPPED: no container runtime");
        return;
    }
    struct Stop<'a>(&'a str);
    impl Drop for Stop<'_> {
        fn drop(&mut self) {
            let _ = Command::new("docker").args(["rm", "-f", self.0]).output();
        }
    }
    let _stop = Stop(&name);
    let port: u16 = docker(&["port", &name, "2201"])
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .rsplit(':')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    std::thread::sleep(Duration::from_secs(1));

    let mut settings = linecon_engine::settings::Settings::ssh("127.0.0.1", port, "lab");
    settings.algorithms(&AlgorithmPolicy::legacy());
    let mut pane = SftpPane::open(
        settings,
        Some(Arc::new(|| {
            Some(Zeroizing::new("lab-password".to_string()))
        })),
        true,
        false,
        Arc::new(|| {}),
    );
    let wait_idle = |pane: &mut SftpPane| {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            assert!(Instant::now() < deadline, "the browser never settled");
            let question = pane.state.lock().unwrap().question.clone();
            match question {
                Some(Question::HostKey(_)) => pane.answer_host_key(HostKeyVerdict::AcceptOnce),
                Some(Question::Prompt(p)) => panic!("asked despite a saved password: {p:?}"),
                None => {}
            }
            let s = pane.state.lock().unwrap();
            if !s.busy && (s.connected || s.error.is_some()) {
                assert_eq!(s.error, None);
                return;
            }
            drop(s);
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    wait_idle(&mut pane);
    assert_eq!(pane.current_path(), "/home/lab");

    let dir = std::env::temp_dir().join(format!("linecon-files-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let local = dir.join("config.bin");
    let content: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(&local, &content).unwrap();
    pane.upload(&local);
    wait_idle(&mut pane);
    assert!(pane.would_overwrite(&local));

    let downloads = dir.join("downloads");
    std::fs::create_dir_all(&downloads).unwrap();
    // Twice, so the second gets a fresh name rather than replacing the first.
    pane.download("config.bin", &downloads);
    wait_idle(&mut pane);
    pane.download("config.bin", &downloads);
    wait_idle(&mut pane);
    assert!(std::fs::read(downloads.join("config.bin")).unwrap() == content);
    assert!(std::fs::read(downloads.join("config (1).bin")).unwrap() == content);
    assert!(!downloads.join("config.bin.part").exists());

    pane.delete("config.bin", false);
    wait_idle(&mut pane);
    assert!(!pane.would_overwrite(&local));
    std::fs::remove_dir_all(dir).unwrap();
}

/// Access set in the window is what the agent server enforces, and it is off
/// until granted.
#[test]
fn agent_access_is_off_until_granted_pane_by_pane() {
    use crate::mcp::{handle, Access};
    use serde_json::json;
    let mut s = sample();
    assert!(
        !s.app.config.mcp_enabled,
        "the server must be off by default"
    );
    s.app
        .add_demo_tab("core-sw1", Some(s.core_sw), SHOW_VERSION);
    let host = s.app.agent_host();
    let mut h = harness(s.app);
    h.run_steps(2);
    let call = |host: &dyn crate::mcp::Host, name: &str, arguments: serde_json::Value| {
        handle(
            host,
            &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": name, "arguments": arguments } }),
        )
        .unwrap()["result"]
            .clone()
    };
    // Off: nothing is shared with the server at all.
    assert_eq!(
        call(host.as_ref(), "list_sessions", json!({}))["content"][0]["text"],
        "[]"
    );

    h.state_mut().config.mcp_enabled = true;
    h.run_steps(2);
    // On, but this pane has not been granted.
    assert_eq!(
        call(host.as_ref(), "list_sessions", json!({}))["content"][0]["text"],
        "[]"
    );

    h.state_mut().set_focused_agent_access(Access::Read);
    h.run_steps(2);
    let listed = call(host.as_ref(), "list_sessions", json!({}));
    let text = listed["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("core-sw1") && text.contains("read"), "{text}");
    let id = serde_json::from_str::<serde_json::Value>(text).unwrap()[0]["session"]
        .as_u64()
        .unwrap();
    let screen = call(host.as_ref(), "read_screen", json!({ "session": id }));
    assert!(screen["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("show version"));
    screenshot(&mut h, "agent");

    // Withdrawing access takes effect, and stopping the server clears everything.
    h.state_mut().set_focused_agent_access(Access::None);
    h.run_steps(2);
    assert_eq!(
        call(host.as_ref(), "list_sessions", json!({}))["content"][0]["text"],
        "[]"
    );
    h.state_mut().set_focused_agent_access(Access::Control);
    h.state_mut().config.mcp_enabled = false;
    h.run_steps(2);
    assert_eq!(
        call(host.as_ref(), "list_sessions", json!({}))["content"][0]["text"],
        "[]"
    );
}

/// Window chrome meets WCAG AA on every theme it can appear on.
#[test]
fn chrome_text_meets_contrast_minimums_on_every_theme() {
    use crate::style::{color, contrast, Chrome};
    for t in crate::themes::builtin() {
        let c = Chrome::from_theme(&t);
        let plain = [
            ("terminal", color(t.background)),
            ("sidebar", c.sidebar),
            ("card", c.raised),
            ("hover", c.hover),
        ];
        for (bg_name, bg) in plain {
            for (name, fg) in [
                ("text", c.text),
                ("muted", c.muted),
                ("accent", c.accent),
                ("danger", c.danger),
                ("success", c.success),
                ("warning", c.warning),
            ] {
                let ratio = contrast(fg, bg);
                assert!(
                    ratio >= 4.5,
                    "{}: {name} on {bg_name} is {ratio:.1}:1",
                    t.name
                );
            }
        }
        for (bg_name, bg) in [("button", c.control), ("selected row", c.selected)] {
            let ratio = contrast(c.text, bg);
            assert!(
                ratio >= 4.5,
                "{}: text on {bg_name} is {ratio:.1}:1",
                t.name
            );
        }
    }
}

/// Every control is big enough to hit, by the WCAG 2.2 AA minimum of 24 points.
#[test]
fn controls_meet_the_minimum_target_size() {
    use egui_kittest::kittest::NodeT;
    let mut s = sample();
    let folder = s.app.config.ancestors(s.core_sw).unwrap()[0].id;
    s.app
        .add_demo_tab("core-sw1", Some(s.core_sw), SHOW_VERSION);
    s.app.add_demo_tab("second", None, TOP);
    s.app.inspect(s.core_sw);
    let _ = folder;
    let mut h = harness(s.app);
    h.run_steps(3);
    // The harness renders at 2x and reports physical pixels.
    const MINIMUM: f64 = 24.0 * 2.0;
    let mut small = Vec::new();
    let mut checked = 0;
    for node in h.root().children_recursive() {
        let n = node.accesskit_node();
        let Some(r) = n.bounding_box() else { continue };
        let role = format!("{:?}", n.role());
        if !matches!(
            role.as_str(),
            "Button" | "TextInput" | "CheckBox" | "ComboBox" | "SpinButton"
        ) {
            continue;
        }
        checked += 1;
        if r.width() < MINIMUM || r.height() < MINIMUM {
            small.push(format!(
                "{role} {:?} {:.0}x{:.0}",
                n.label().unwrap_or_default(),
                r.width() / 2.0,
                r.height() / 2.0
            ));
        }
    }
    assert!(checked > 10, "only found {checked} controls");
    assert!(small.is_empty(), "too small:\n{}", small.join("\n"));
}

#[test]
fn moving_to_a_field_selects_its_text_so_typing_replaces_it() {
    let mut s = sample();
    let id = s.core_sw;
    s.app.inspect(id);
    let mut h = harness(s.app);
    h.run_steps(3);
    let host = h
        .query_all_by_role(egui::accesskit::Role::TextInput)
        .find(|n| n.value().as_deref() == Some("10.20.0.2"));
    host.expect("the host field").focus();
    h.run_steps(3);
    let target = h
        .query_all_by_role(egui::accesskit::Role::TextInput)
        .find(|n| n.value().as_deref() == Some("10.20.0.2"));
    target.expect("the host field").type_text("sw9");
    h.run_steps(3);
    let session = h.state().config.session(id).unwrap();
    assert_eq!(
        session.target.host, "sw9",
        "typing should replace, not add to, the old value"
    );
}

#[derive(Default)]
struct MemoryStore(std::collections::HashMap<String, String>);

impl linecon_config::secrets::SecretStore for MemoryStore {
    fn get(
        &self,
        key: &str,
    ) -> Result<Option<zeroize::Zeroizing<String>>, linecon_config::secrets::SecretError> {
        Ok(self.0.get(key).map(|v| zeroize::Zeroizing::new(v.clone())))
    }
    fn set(&mut self, key: &str, value: &str) -> Result<(), linecon_config::secrets::SecretError> {
        self.0.insert(key.to_string(), value.to_string());
        Ok(())
    }
    fn remove(&mut self, key: &str) -> Result<(), linecon_config::secrets::SecretError> {
        self.0.remove(key);
        Ok(())
    }
}

#[test]
fn a_login_is_saved_once_the_device_accepts_it() {
    let mut s = sample();
    s.app.use_vault(Box::<MemoryStore>::default());
    s.app.add_demo_tab("core-sw1", Some(s.core_sw), b"");
    s.app.demo_remember(Some("netops"), Some("hunter2"));
    let id = s.core_sw;
    let mut h = harness(s.app);
    h.run_steps(2);
    let app = h.state();
    let identity = app
        .config
        .resolve(id)
        .unwrap()
        .identity(&app.config)
        .unwrap()
        .clone();
    assert_eq!(identity.username, "netops");
    assert!(identity.has_password);
    assert_eq!(
        app.vault_get(&identity.secret_key()).as_deref(),
        Some("hunter2")
    );
    // The secret must not reach the config file.
    assert!(!toml_of(app).contains("hunter2"));
}

fn toml_of(app: &App) -> String {
    let dir = std::env::temp_dir().join(format!(
        "linecon-toml-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let path = dir.join("c.toml");
    linecon_config::save(&path, &app.config).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::remove_dir_all(dir).unwrap();
    text
}

#[test]
fn a_rejected_login_is_not_saved() {
    let mut s = sample();
    s.app.use_vault(Box::<MemoryStore>::default());
    s.app.add_demo_tab("core-sw1", Some(s.core_sw), b"");
    s.app.demo_remember(Some("netops"), Some("wrong"));
    s.app.demo_close_focused_connection();
    let id = s.core_sw;
    let identities = s.app.config.identities.len();
    let mut h = harness(s.app);
    h.run_steps(2);
    let app = h.state();
    assert_eq!(app.config.identities.len(), identities);
    assert!(app
        .config
        .resolve(id)
        .unwrap()
        .identity(&app.config)
        .is_none());
}

#[test]
fn remembering_a_quick_connection_saves_it_as_a_session() {
    let mut s = sample();
    s.app.use_vault(Box::<MemoryStore>::default());
    s.app.add_demo_tab("kaiju", None, b"");
    s.app
        .demo_set_settings(linecon_engine::settings::Settings::ssh(
            "kaiju", 22, "tonhe",
        ));
    s.app.demo_remember(None, Some("pw"));
    let mut h = harness(s.app);
    h.run_steps(2);
    let app = h.state();
    let id = app
        .focused_session()
        .expect("the pane now belongs to a saved session");
    assert_eq!(app.config.session(id).unwrap().target.host, "kaiju");
    let identity = app
        .config
        .resolve(id)
        .unwrap()
        .identity(&app.config)
        .unwrap()
        .clone();
    assert_eq!(identity.username, "tonhe");
    assert_eq!(app.vault_get(&identity.secret_key()).as_deref(), Some("pw"));
}

#[test]
fn logins_can_be_made_and_edited_from_the_session_itself() {
    let mut s = sample();
    let id = s.core_sw;
    s.app.inspect(id);
    let mut h = harness(s.app);
    h.run_steps(3);
    assert!(
        h.query_by_label("Connect").is_some(),
        "no way to connect from the editor"
    );
    h.get_by_label("New login").click();
    h.run_steps(3);
    assert!(h
        .state()
        .config
        .resolve(id)
        .unwrap()
        .identity(&h.state().config)
        .is_some());
    assert!(h.query_by_label("Username").is_some());
    assert!(h.query_by_label("Save password").is_some());
    screenshot(&mut h, "login");
}

#[test]
fn deleting_asks_first() {
    let mut s = sample();
    s.app.ask_delete_first_folder();
    let folder = s.app.config.nodes[0].id();
    let mut h = harness(s.app);
    h.run_steps(3);
    assert!(
        h.state().config.find(folder).is_some(),
        "deleted without asking"
    );
    assert!(h.query_by_label("Keep").is_some());
    h.get_by_label("Keep").click();
    h.run_steps(3);
    assert!(h.state().config.find(folder).is_some());
}

/// Closing a dead session from its card used to overwrite another tab's layout
/// with the closed pane, leaving a blank tab that could not be closed.
#[test]
fn closing_a_disconnected_tab_leaves_the_others_intact() {
    let mut s = sample();
    s.app.add_demo_tab("first", None, b"first tab text");
    s.app.add_demo_tab("second", None, b"second tab text");
    s.app.add_demo_tab("third", None, b"third tab text");
    s.app.demo_close_focused_connection();
    let mut h = harness(s.app);
    h.run_steps(3);
    h.get_by_label("Close").click();
    h.run_steps(3);
    assert_eq!(h.state().tab_count(), 2);
    assert_eq!(h.state().pane_count(), 2);
    h.state().assert_tabs_consistent();
    assert!(h.state().focused_screen_contains("tab text"));

    // Closing everything that is left also works, down to none.
    h.state_mut().close_focused();
    h.run_steps(2);
    h.state().assert_tabs_consistent();
    h.state_mut().close_focused();
    h.run_steps(2);
    assert_eq!(h.state().tab_count(), 0);
}

#[test]
fn a_tab_pointing_at_a_missing_pane_repairs_itself() {
    let mut s = sample();
    s.app.add_demo_tab("first", None, b"");
    s.app.add_demo_tab("second", None, b"");
    s.app.break_a_tab_for_test();
    let mut h = harness(s.app);
    h.run_steps(2);
    assert_eq!(h.state().tab_count(), 1);
}

#[test]
fn enter_answers_the_password_prompt() {
    let mut s = sample();
    s.app.add_demo_tab("core-sw1", Some(s.core_sw), b"");
    s.app.demo_pending(Pending::Prompt(Prompt {
        name: None,
        instruction: None,
        from_server: false,
        fields: vec![PromptField {
            text: "admin@sw1's password: ".into(),
            echo: false,
        }],
    }));
    let mut h = harness(s.app);
    h.run_steps(3);
    h.get_by_role(egui::accesskit::Role::PasswordInput)
        .type_text("secret");
    h.run_steps(1);
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    assert!(
        h.state().focused_pending_is_none(),
        "Enter did not answer the prompt"
    );
}

fn listening_peer() -> u16 {
    use std::io::Read;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            std::thread::spawn(move || {
                let mut buf = [0u8; 64];
                while matches!(stream.read(&mut buf), Ok(n) if n > 0) {}
            });
        }
    });
    port
}

fn settle(h: &mut Harness<'_, App>, done: impl Fn(&App) -> bool) -> bool {
    for _ in 0..300 {
        h.step();
        if done(h.state()) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    false
}

#[test]
fn quick_connections_are_kept_once_they_work_without_duplicates() {
    let port = listening_peer();
    let address = format!("raw://127.0.0.1:{port}");
    let mut app = App::new(location());
    app.quick_open_like_the_sidebar(&address);
    let mut h = harness(app);
    assert!(
        settle(&mut h, |a| a.quick_folder_sessions().len() == 1),
        "never saved"
    );
    assert_eq!(h.state().quick_folder_sessions(), ["127.0.0.1"]);

    // Same place again: one entry, not two.
    h.state_mut().quick_open_like_the_sidebar(&address);
    assert!(
        settle(&mut h, |a| a.focused_session().is_some()),
        "second connection not filed"
    );
    assert_eq!(h.state().quick_folder_sessions().len(), 1);
    assert!(h.query_by_label("Quick sessions").is_some());
}

#[test]
fn a_quick_connection_that_fails_is_not_saved() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let mut app = App::new(location());
    app.quick_open_like_the_sidebar(&format!("raw://127.0.0.1:{port}"));
    let mut h = harness(app);
    for _ in 0..50 {
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(h.state().quick_folder_sessions().is_empty());
}

#[test]
fn the_sidebar_can_be_hidden_and_brought_back() {
    let s = sample();
    let mut h = harness(s.app);
    h.run_steps(3);
    assert!(h.query_by_label("Bench console").is_some());
    h.get_by_label("‹").click();
    h.run_steps(3);
    assert!(h.query_by_label("Bench console").is_none(), "still showing");
    h.get_by_label("›").click();
    h.run_steps(3);
    assert!(
        h.query_by_label("Bench console").is_some(),
        "did not come back"
    );
}

#[test]
fn a_session_bundle_moves_between_installs_through_the_app() {
    let s = sample();
    let text = linecon_config::import::export_bundle(&s.app.config, None, true).unwrap();
    let mut other = App::new(location());
    other.import_bundle_for_test(&text);
    let mut h = harness(other);
    h.run_steps(3);
    assert!(h.query_by_label("Imported sessions").is_some());
    assert!(
        h.query_by_label("core-sw1").is_some(),
        "the tree did not arrive"
    );
    assert!(
        h.query_by_label("Core switches").is_some(),
        "folders did not arrive"
    );
}

/// Real devices through the window's own code path, which decides about RSA
/// separately from the command line. Run by tests/lab/README.md; needs the lab up.
#[test]
#[ignore = "needs the device lab"]
fn real_devices_through_the_window_path() {
    use crate::pane::{Pane, PaneSpec, Status};
    use linecon_engine::HostKeyVerdict;
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use zeroize::Zeroizing;

    let lab = std::env::var("LINECON_LAB").expect("LINECON_LAB=name,host,user,password;...");
    for device in lab.split(';') {
        let parts: Vec<&str> = device.split(',').collect();
        let [name, host, user, password] = parts[..] else {
            panic!("bad entry: {device}")
        };
        let config = linecon_config::Config::default();
        let resolved = config.resolve_root();
        let target = Target {
            host: host.into(),
            ..Default::default()
        };
        let password = password.to_string();
        let spec = PaneSpec {
            name: name.into(),
            session_id: None,
            settings: resolved.engine_settings(&target, user),
            policy: resolved.algorithm_policy(),
            log_dir: None,
            scrollback: 1000,
            copy_on_select: true,
            auto_reconnect: false,
            accept_weak: resolved.legacy_algorithms.value,
            trust_new_host_keys: false,
            password: Some(Arc::new(move || Some(Zeroizing::new(password.clone())))),
        };
        assert!(!spec.accept_weak, "this must test default settings");
        let pane = Pane::new(spec, Arc::new(|| {}));
        let deadline = Instant::now() + Duration::from_secs(40);
        while pane.status() != Status::Live {
            assert!(Instant::now() < deadline, "{name}: {:?}", pane.status());
            let pending = pane.shared.pending.lock().unwrap().clone();
            match pending {
                Some(Pending::HostKey(_)) => pane.answer_hostkey(HostKeyVerdict::AcceptOnce),
                Some(Pending::Prompt(p)) => panic!("{name} asked despite a saved password: {p:?}"),
                None => {}
            }
            assert!(
                !matches!(pane.status(), Status::Closed { .. }),
                "{name}: {:?}",
                pane.status()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        pane.send(b"show version\r");
        let deadline = Instant::now() + Duration::from_secs(20);
        let shown = loop {
            let text = pane.shared.core.lock().unwrap().text_with_scrollback(500);
            if text.to_lowercase().contains("version") && text.lines().count() > 3 {
                break text;
            }
            assert!(Instant::now() < deadline, "{name} showed: {text}");
            std::thread::sleep(Duration::from_millis(50));
        };
        println!(
            "PASS  {name}: {}",
            shown
                .lines()
                .find(|l| l.to_lowercase().contains("version"))
                .unwrap_or("")
        );
    }
}

/// The same username on two devices must not mean the same password. Sending one
/// device's password to another is the failure this guards against.
#[test]
fn a_saved_password_stays_with_its_own_session() {
    use linecon_engine::settings::Settings;
    let mut s = sample();
    s.app.use_vault(Box::<MemoryStore>::default());
    s.app.add_demo_tab("first", None, b"");
    s.app
        .demo_set_settings(Settings::ssh("10.0.0.1", 22, "admin"));
    s.app.demo_remember(None, Some("first-password"));
    let mut h = harness(s.app);
    h.run_steps(2);
    let first = h.state().focused_session().expect("first saved");

    h.state_mut().add_demo_tab("second", None, b"");
    h.state_mut()
        .demo_set_settings(Settings::ssh("10.0.0.2", 22, "admin"));
    h.state_mut().demo_remember(None, None);
    h.run_steps(2);
    let second = h.state().focused_session().expect("second saved");
    assert_ne!(first, second);
    assert_eq!(
        h.state().demo_session_login_password(second),
        None,
        "the second device would have been sent the first device's password"
    );

    h.state_mut().add_demo_tab("third", None, b"");
    h.state_mut()
        .demo_set_settings(Settings::ssh("10.0.0.2", 22, "admin"));
    h.state_mut().demo_remember(None, Some("second-password"));
    h.run_steps(2);
    assert_eq!(
        h.state().demo_session_login_password(first).as_deref(),
        Some("first-password")
    );
    assert_eq!(
        h.state().demo_session_login_password(second).as_deref(),
        Some("second-password")
    );
}

/// Enter in the quick connect box is for the box. It must not reach the device.
#[test]
fn the_enter_that_opens_a_quick_connection_is_not_sent_to_it() {
    use std::io::Read;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 64];
        while let Ok(n) = stream.read(&mut buf) {
            if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    let mut h = harness(App::new(location()));
    h.run_steps(3);
    h.get_by_role(egui::accesskit::Role::TextInput).focus();
    h.run_steps(2);
    h.get_by_role(egui::accesskit::Role::TextInput)
        .type_text(&format!("raw://127.0.0.1:{port}"));
    h.run_steps(1);
    h.key_press(egui::Key::Enter);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while std::time::Instant::now() < deadline {
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(h.state().tab_count(), 1, "the connection did not open");
    let got: Vec<u8> = rx.try_iter().flatten().collect();
    assert!(got.is_empty(), "sent to the device on connect: {got:?}");
}

fn app_with_updates(checks: Option<bool>) -> App {
    let mut app = App::new(location());
    app.set_update_key([1; 32]);
    app.set_update_checks(checks);
    app
}

/// Builds without a key, including development builds, show no trace of updating.
#[test]
fn with_no_update_key_there_is_no_sign_of_updating() {
    let mut h = harness(App::new(location()));
    h.run_steps(3);
    assert!(h
        .query_by_label("Check for updates automatically?")
        .is_none());
    h.state_mut().open_palette("updates");
    h.run_steps(3);
    assert!(h.query_by_label("Check for updates").is_none());
}

/// The question is asked once, and declining means no check is ever made.
#[test]
fn the_first_launch_asks_about_update_checks_and_remembers() {
    let mut h = harness(app_with_updates(None));
    h.run_steps(3);
    h.get_by_label("Check for updates automatically?");
    h.get_by_label("Don't check").click();
    h.run_steps(3);
    assert_eq!(h.state().update_checks(), Some(false));
    assert!(h
        .query_by_label("Check for updates automatically?")
        .is_none());
}

#[test]
fn the_update_question_does_not_take_the_keyboard() {
    let mut h = harness(app_with_updates(None));
    h.run_steps(3);
    h.key_press(egui::Key::Enter);
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert_eq!(h.state().update_checks(), None);
}

#[test]
fn an_update_can_be_refused_and_nothing_happens() {
    let mut h = harness(app_with_updates(Some(false)));
    h.state_mut().offer_update("0.0.9", true);
    h.run_steps(3);
    h.get_by_label("linecon0 0.0.9 is available");
    h.get_by_label("Update");
    h.get_by_label("Later").click();
    h.run_steps(3);
    assert!(h.query_by_label("linecon0 0.0.9 is available").is_none());
}

/// Un-notarized Mac releases, for one, can only be installed by hand.
#[test]
fn an_update_that_cannot_install_offers_only_the_download_page() {
    let mut h = harness(app_with_updates(Some(false)));
    h.state_mut().offer_update("0.0.9", false);
    h.run_steps(3);
    assert!(h.query_by_label("Update").is_none());
    h.get_by_label("Open download page");
}

#[test]
fn restarting_says_how_many_sessions_it_closes() {
    let mut h = harness(app_with_updates(Some(false)));
    h.state_mut().add_demo_tab("one", None, b"");
    h.state_mut().add_demo_tab("two", None, b"");
    h.state_mut().update_installed("0.0.9");
    h.run_steps(3);
    h.get_by_label_contains("closes your 2 open sessions");
    h.get_by_label("Restart now");
}

/// Writes a SecureCRT export whose passwords are the given encrypted values.
fn securecrt_export(name: &str, password: &str) -> std::path::PathBuf {
    let file =
        std::env::temp_dir().join(format!("linecon-crt-ui-{}-{name}.xml", std::process::id()));
    std::fs::write(
        &file,
        format!(
            r#"<VanDyke version="3.0"><key name="Sessions"><key name="sw1">
            <dword name="Is Session">1</dword><string name="Protocol Name">SSH2</string>
            <string name="Hostname">10.1.1.1</string><string name="Username">admin</string>
            <string name="Password V2">{password}</string></key></key></VanDyke>"#
        ),
    )
    .unwrap();
    file
}

fn run_until_passwords_settle(h: &mut Harness<'static, App>) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    h.run_steps(2);
    while h.state().imported_passwords_busy() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
        h.run_steps(1);
    }
    h.run_steps(3);
}

const CRT_NO_PASSPHRASE: &str = "02:a946feca74f2916b52e36e1458fa1a6647b1af207ac92ee8b3273d4f6e54f8c85d5a0dcd7431cd38b8cab7f85133bddf";
const CRT_WITH_PASSPHRASE: &str = "02:ada4e6e1709568d558ffc67a7cdd4d24b9e7187c7635b2550da50d6e6fb3acc72cd99789db9b703fd3e9daeb65dd161f";

#[test]
fn securecrt_passwords_land_in_the_secret_store() {
    let mut app = App::new(location());
    app.use_vault(Box::<MemoryStore>::default());
    let file = securecrt_export("plain", CRT_NO_PASSPHRASE);
    app.import_for_test(&file);
    let mut h = harness(app);
    run_until_passwords_settle(&mut h);
    let app = h.state();
    let identity = app.config_for_test().identities.first().unwrap().clone();
    assert!(identity.has_password);
    assert_eq!(
        app.vault_get(&identity.secret_key()).as_deref(),
        Some("hunter2")
    );
    let saved = std::fs::read_to_string(app.config_file_for_test()).unwrap_or_default();
    assert!(!saved.contains("hunter2"), "plain text in the config file");
    let _ = std::fs::remove_file(file);
}

#[test]
fn a_securecrt_passphrase_is_asked_for_and_can_be_retried() {
    let mut app = App::new(location());
    app.use_vault(Box::<MemoryStore>::default());
    let file = securecrt_export("phrase", CRT_WITH_PASSPHRASE);
    app.import_for_test(&file);
    let mut h = harness(app);
    run_until_passwords_settle(&mut h);
    h.get_by_label("SecureCRT configuration passphrase");
    h.get_by_role(egui::accesskit::Role::PasswordInput)
        .type_text("wrong");
    h.run_steps(1);
    h.get_by_label("Decrypt").click();
    run_until_passwords_settle(&mut h);
    h.get_by_label("That passphrase does not open them.");
    h.get_by_role(egui::accesskit::Role::PasswordInput)
        .type_text("config phrase");
    h.run_steps(1);
    h.get_by_label("Decrypt").click();
    run_until_passwords_settle(&mut h);
    assert!(h
        .query_by_label("SecureCRT configuration passphrase")
        .is_none());
    let app = h.state();
    let identity = app.config_for_test().identities.first().unwrap().clone();
    assert_eq!(
        app.vault_get(&identity.secret_key()).as_deref(),
        Some("hunter2")
    );
    let _ = std::fs::remove_file(file);
}

#[test]
fn skipping_the_securecrt_passphrase_keeps_the_sessions() {
    let mut app = App::new(location());
    app.use_vault(Box::<MemoryStore>::default());
    let file = securecrt_export("skip", CRT_WITH_PASSPHRASE);
    app.import_for_test(&file);
    let mut h = harness(app);
    run_until_passwords_settle(&mut h);
    h.get_by_label("Skip").click();
    h.run_steps(3);
    assert!(h
        .query_by_label("SecureCRT configuration passphrase")
        .is_none());
    let identity = h
        .state()
        .config_for_test()
        .identities
        .first()
        .unwrap()
        .clone();
    assert!(!identity.has_password);
    let _ = std::fs::remove_file(file);
}

/// Typing must stay with the device it was going to.
#[test]
fn closing_an_earlier_tab_keeps_the_same_tab_active() {
    let mut app = App::new(location());
    app.add_demo_tab("a", None, b"AAAA");
    app.add_demo_tab("b", None, b"BBBB");
    app.add_demo_tab("c", None, b"CCCC");
    app.set_active_tab_for_test(1);
    app.close_tab_for_test(0);
    assert_eq!(app.active_tab_for_test(), 0);
    assert!(app.focused_screen_contains("BBBB"));
    // Closing the last tab while it is active still lands somewhere valid.
    app.set_active_tab_for_test(1);
    app.close_tab_for_test(1);
    assert_eq!(app.active_tab_for_test(), 0);
    assert!(app.focused_screen_contains("BBBB"));
}

/// A login inherited from a folder is used by everything under that folder.
#[test]
fn remembering_on_an_inherited_login_does_not_share_the_password() {
    let mut app = App::new(location());
    app.use_vault(Box::<MemoryStore>::default());
    let folder = app.config.add_folder(None, "site").unwrap();
    let shared = app.config.add_identity("site login", "netops");
    app.config.overrides_mut(folder).unwrap().identity = Some(shared);
    let sw1 = app
        .config
        .add_session(Some(folder), "sw1", target("10.0.0.1"))
        .unwrap();
    let sw2 = app
        .config
        .add_session(Some(folder), "sw2", target("10.0.0.2"))
        .unwrap();
    app.add_demo_tab("sw1", Some(sw1), b"");
    app.demo_remember(Some("netops"), Some("hunter2"));
    let mut h = harness(app);
    h.run_steps(2);
    let app = h.state();
    assert!(
        !app.config.identity(shared).unwrap().has_password,
        "the password went on the login sw2 also uses"
    );
    let on_sw1 = app.config.resolve(sw1).unwrap();
    assert!(on_sw1.identity(&app.config).unwrap().has_password);
    let on_sw2 = app.config.resolve(sw2).unwrap();
    assert!(!on_sw2.identity(&app.config).unwrap().has_password);
}

/// A saved password belongs to its user, so a different typed user must not send it.
#[test]
fn quick_connect_as_another_user_does_not_use_the_global_password() {
    let mut app = App::new(location());
    app.use_vault(Box::<MemoryStore>::default());
    let global = app.config.add_identity("global", "root");
    app.config.identity_mut(global).unwrap().has_password = true;
    app.config.settings.identity = Some(global);
    assert!(!app.spec_has_password_for_test("bob"));
    assert!(app.spec_has_password_for_test("root"));
}

/// A new key type on a known host is not the same as a new host.
#[test]
fn a_host_known_with_a_different_key_says_so() {
    let mut s = sample();
    s.app.add_demo_tab("core-sw1", Some(s.core_sw), b"");
    s.app.demo_pending(Pending::HostKey(HostKeyPrompt {
        key_type: "ecdsa-sha2-nistp256".into(),
        key: "abc".into(),
        port: 22,
        description: "The host key is not cached for this server".into(),
        known_types: vec!["ssh-ed25519".into()],
        changed: false,
    }));
    let mut h = harness(s.app);
    h.run_steps(3);
    h.get_by_label("This host is known with a different key");
    assert!(h.query_by_label("Unknown host key").is_none());
}

fn agent_call(
    host: &dyn crate::mcp::Host,
    name: &str,
    arguments: serde_json::Value,
) -> (bool, String) {
    let reply = crate::mcp::handle(
        host,
        &serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": name, "arguments": arguments } }),
    )
    .unwrap();
    (
        reply["result"]["isError"].as_bool().unwrap_or(false),
        reply["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_string(),
    )
}

fn live_agent_pane() -> (
    Harness<'static, App>,
    std::sync::Arc<dyn crate::mcp::Host>,
    u64,
) {
    let mut s = sample();
    s.app.add_demo_tab("core-sw1", Some(s.core_sw), b"");
    s.app.config.mcp_enabled = true;
    s.app.set_focused_live_for_test(true);
    s.app.set_focused_agent_access(crate::mcp::Access::Ask);
    let host = s.app.agent_host();
    let mut h = harness(s.app);
    h.run_steps(3);
    let id = serde_json::from_str::<serde_json::Value>(
        &agent_call(host.as_ref(), "list_sessions", serde_json::json!({})).1,
    )
    .unwrap()[0]["session"]
        .as_u64()
        .unwrap();
    (h, host, id)
}

#[test]
fn an_asking_pane_waits_for_a_person_to_decide() {
    use serde_json::json;
    let (mut h, host, id) = live_agent_pane();
    let (error, text) = agent_call(
        host.as_ref(),
        "send_text",
        json!({ "session": id, "text": "reload\n" }),
    );
    assert!(!error, "{text}");
    assert!(text.contains("Waiting"));
    h.run_steps(3);
    h.get_by_label("An agent wants to send this to core-sw1");
    assert_eq!(
        agent_call(host.as_ref(), "get_proposal", json!({ "proposal": 1 })).1,
        "pending"
    );
    h.get_by_label("Deny").click();
    h.run_steps(3);
    assert_eq!(
        agent_call(host.as_ref(), "get_proposal", json!({ "proposal": 1 })).1,
        "denied"
    );

    agent_call(
        host.as_ref(),
        "send_text",
        json!({ "session": id, "text": "show clock\n" }),
    );
    h.run_steps(3);
    h.get_by_label("Approve").click();
    h.run_steps(3);
    assert_eq!(
        agent_call(host.as_ref(), "get_proposal", json!({ "proposal": 2 })).1,
        "sent"
    );
    assert!(h.query_by_label("Approve").is_none());
}

#[test]
fn a_disconnect_ends_proposals_and_takes_back_full_control() {
    use crate::mcp::Access;
    use serde_json::json;
    let (mut h, host, id) = live_agent_pane();
    agent_call(
        host.as_ref(),
        "send_text",
        json!({ "session": id, "text": "reload\n" }),
    );
    h.state_mut().set_focused_agent_access(Access::Control);
    h.run_steps(3);
    h.state_mut().set_focused_live_for_test(false);
    h.run_steps(3);
    assert_eq!(
        agent_call(host.as_ref(), "get_proposal", json!({ "proposal": 1 })).1,
        "expired"
    );
    assert_eq!(h.state().focused_agent_access_for_test(), Access::Ask);
    // Coming back does not restore it.
    h.state_mut().set_focused_live_for_test(true);
    h.run_steps(3);
    assert_eq!(h.state().focused_agent_access_for_test(), Access::Ask);
}

#[test]
fn proposal_keys_cannot_approve() {
    use serde_json::json;
    let (mut h, host, id) = live_agent_pane();
    agent_call(
        host.as_ref(),
        "send_text",
        json!({ "session": id, "text": "reload\n" }),
    );
    h.run_steps(3);
    h.key_press(egui::Key::Enter);
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert_eq!(
        agent_call(host.as_ref(), "get_proposal", json!({ "proposal": 1 })).1,
        "pending"
    );
}

/// A key meant for a field must not also reach the device.
#[test]
fn closing_the_search_bar_does_not_send_escape_to_the_device() {
    use std::io::Read;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 64];
        while let Ok(n) = stream.read(&mut buf) {
            if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    let mut h = harness(App::new(location()));
    h.run_steps(3);
    h.state_mut().quick_open(&format!("raw://127.0.0.1:{port}"));
    let settle = |h: &mut Harness<'static, App>| {
        let until = std::time::Instant::now() + std::time::Duration::from_millis(600);
        while std::time::Instant::now() < until {
            h.step();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    };
    settle(&mut h);
    h.state_mut().open_search_for_test();
    h.run_steps(2);
    h.get_all_by_role(egui::accesskit::Role::TextInput)
        .last()
        .unwrap()
        .focus();
    h.run_steps(2);
    h.key_press(egui::Key::Escape);
    settle(&mut h);
    assert!(!h.state().search_open_for_test(), "the bar stayed open");
    let got: Vec<u8> = rx.try_iter().flatten().collect();
    assert!(got.is_empty(), "sent to the device: {got:?}");
}

/// A second window must not erase what the first one saves.
#[test]
fn a_second_window_cannot_overwrite_the_settings() {
    let shared = location();
    let mut first = App::new(shared.clone());
    first.config.add_folder(None, "from the first window");
    first.mark_dirty_for_test();
    first.save_for_test();
    let mut second = App::new(shared.clone());
    second.config.add_folder(None, "from the second window");
    second.mark_dirty_for_test();
    second.save_for_test();
    let saved = std::fs::read_to_string(shared.config_file()).unwrap();
    assert!(saved.contains("from the first window"));
    assert!(!saved.contains("from the second window"));
    // Once the first has gone, the next window owns it.
    drop(first);
    drop(second);
    let mut third = App::new(shared.clone());
    third.config.add_folder(None, "from the third window");
    third.mark_dirty_for_test();
    third.save_for_test();
    let saved = std::fs::read_to_string(shared.config_file()).unwrap();
    assert!(saved.contains("from the third window"));
}

/// One failure must not lose every later edit.
#[test]
fn a_failed_save_is_tried_again() {
    let place = location();
    std::fs::create_dir_all(place.config_file()).unwrap(); // a folder in the way of the file
    let mut app = App::new(place.clone());
    app.config_error_clear_for_test();
    app.config.add_folder(None, "kept");
    app.mark_dirty_for_test();
    app.save_for_test();
    assert!(app.save_error_for_test().is_some());
    std::fs::remove_dir_all(place.config_file()).unwrap();
    app.forget_failed_save_for_test();
    app.save_for_test();
    assert!(app.save_error_for_test().is_none());
    let saved = std::fs::read_to_string(place.config_file()).unwrap();
    assert!(saved.contains("kept"));
}

/// An OpenSSH key asks for its passphrase through the same card as any other question.
#[test]
fn an_encrypted_openssh_key_asks_before_connecting() {
    use crate::pane::{Pane, Pending, Status};
    let dir = std::env::temp_dir().join(format!("linecon-key-card-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let key = dir.join("id");
    let made = std::process::Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "sesame", "-f"])
        .arg(&key)
        .status();
    if !made.is_ok_and(|s| s.success()) {
        eprintln!("SKIPPED: no ssh-keygen");
        return;
    }
    crate::openssh_keys::set_cache_dir(
        std::env::temp_dir().join(format!("linecon-key-cache-{}", std::process::id())),
    );
    let app = App::new(location());
    let target = Target {
        host: "127.0.0.1".to_string(),
        port: Some(1),
        ..Default::default()
    };
    let mut spec = app.spec_for_test("sw", &target);
    spec.settings
        .set("PublicKeyFile", key.display().to_string());
    let mut pane = Pane::new(spec, std::sync::Arc::new(|| {}));
    pane.connect();
    let question = |pane: &Pane| match &*pane.shared.pending.lock().unwrap() {
        Some(Pending::Prompt(p)) => Some(p.instruction.clone().unwrap_or_default()),
        _ => None,
    };
    assert!(question(&pane).expect("a question").contains("protected"));
    assert_eq!(pane.status(), Status::Connecting);

    pane.answer_prompt(&["wrong"]);
    pane.tick_reconnect();
    assert!(question(&pane)
        .expect("asks again")
        .contains("does not open"));

    pane.answer_prompt(&["sesame"]);
    pane.tick_reconnect();
    assert!(question(&pane).is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

/// A name that will not resolve fails that session, without holding up the window.
#[test]
fn a_failed_lookup_closes_only_that_session() {
    use crate::pane::{Pane, Status};
    fn slow_failure(_: &str, _: u16) -> std::io::Result<Vec<std::net::IpAddr>> {
        std::thread::sleep(std::time::Duration::from_secs(2));
        Err(std::io::Error::other("no such host"))
    }
    let app = App::new(location());
    let target = Target {
        host: "sw1.example".to_string(),
        ..Default::default()
    };
    let spec = app.spec_for_test("sw", &target);
    let mut pane = Pane::new(spec, std::sync::Arc::new(|| {}));
    pane.look_up = slow_failure;
    let started = std::time::Instant::now();
    pane.connect();
    assert!(
        started.elapsed() < std::time::Duration::from_secs(1),
        "connect waited for the lookup"
    );
    assert_eq!(pane.status(), Status::Connecting);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while pane.status() == Status::Connecting && std::time::Instant::now() < deadline {
        pane.tick_reconnect();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let Status::Closed { reason } = pane.status() else {
        panic!("still connecting");
    };
    let reason = reason.unwrap_or_default();
    assert!(reason.contains("sw1.example"), "{reason}");
}

#[test]
fn a_slow_old_lookup_does_not_replace_the_newest_answer() {
    use crate::pane::{Pane, Status};
    use std::sync::atomic::{AtomicUsize, Ordering};
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    fn first_is_slow(_: &str, _: u16) -> std::io::Result<Vec<std::net::IpAddr>> {
        if CALLS.fetch_add(1, Ordering::SeqCst) == 0 {
            std::thread::sleep(std::time::Duration::from_millis(800));
            return Err(std::io::Error::other("old answer"));
        }
        Err(std::io::Error::other("new answer"))
    }
    let app = App::new(location());
    let target = Target {
        host: "sw1.example".to_string(),
        ..Default::default()
    };
    let mut pane = Pane::new(app.spec_for_test("sw", &target), std::sync::Arc::new(|| {}));
    pane.look_up = first_is_slow;
    pane.connect();
    // The first connect's lookup has to be the slow one.
    while CALLS.load(Ordering::SeqCst) == 0 {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    pane.connect();
    // Both answers are in before the window looks, the old one last.
    std::thread::sleep(std::time::Duration::from_millis(1500));
    pane.tick_reconnect();
    let Status::Closed { reason } = pane.status() else {
        panic!("the newest answer was lost and the session waits forever");
    };
    let reason = reason.unwrap_or_default();
    assert!(reason.contains("new answer"), "{reason}");
}

#[test]
fn a_jump_host_is_looked_up_before_the_shared_thread_sees_it() {
    use crate::pane::{Pane, Status};
    static ASKED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    fn no_such_host(host: &str, _: u16) -> std::io::Result<Vec<std::net::IpAddr>> {
        ASKED.lock().unwrap().push(host.to_string());
        Err(std::io::Error::other("no such host"))
    }
    let app = App::new(location());
    let target = Target {
        host: "sw1.example".to_string(),
        ..Default::default()
    };
    let mut spec = app.spec_for_test("sw", &target);
    let hop = spec.settings.clone();
    spec.settings.via_jump_host(
        &linecon_engine::settings::JumpHost::parse("ops@bastion.example").unwrap(),
        &hop,
    );
    let mut pane = Pane::new(spec, std::sync::Arc::new(|| {}));
    pane.look_up = no_such_host;
    pane.connect();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while pane.status() == Status::Connecting && std::time::Instant::now() < deadline {
        pane.tick_reconnect();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let Status::Closed { reason } = pane.status() else {
        panic!("still connecting");
    };
    let reason = reason.unwrap_or_default();
    assert!(
        reason.contains("could not look up bastion.example"),
        "{reason}"
    );
    assert_eq!(*ASKED.lock().unwrap(), ["bastion.example"]);
}

#[test]
fn the_finder_searches_sessions_by_name_address_and_folder() {
    let mut s = sample();
    let folder = s.app.config.add_folder(None, "Datacenter").unwrap();
    s.app
        .config
        .add_session(Some(folder), "edge-fw", target("172.16.9.9"))
        .unwrap();
    s.app.open_connection_finder_with("edge");
    let mut h = harness(s.app);
    h.run_steps(3);
    // The sidebar always shows it, so a match is a second one.
    let shown = |h: &Harness<'static, App>| h.get_all_by_label("edge-fw").count();
    assert_eq!(shown(&h), 2);
    // Only connections: commands are for the palette.
    assert!(h.query_by_label("Split right").is_none());
    for query in ["172.16", "datacenter", "EDGE-FW"] {
        h.state_mut().open_connection_finder_with(query);
        h.run_steps(3);
        assert_eq!(shown(&h), 2, "{query}");
    }
    h.state_mut()
        .open_connection_finder_with("nothing-like-this-here x");
    h.run_steps(3);
    assert_eq!(shown(&h), 1);
}

/// A saved match comes first, so Enter takes what was searched for.
#[test]
fn enter_in_the_finder_opens_the_saved_session_before_a_new_one() {
    let mut s = sample();
    let folder = s.app.config.add_folder(None, "Datacenter").unwrap();
    s.app
        .config
        .add_session(Some(folder), "edge-fw", target("127.0.0.1"))
        .unwrap();
    s.app.open_connection_finder_with("edge-fw");
    let mut h = harness(s.app);
    h.run_steps(3);
    h.get_by_label("Connect to edge-fw");
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert_eq!(h.state().tab_count(), 1);
    assert!(!h.state().focused_is_quick_for_test());
}

#[test]
fn the_finder_connects_to_something_new() {
    let app = App::new(location());
    let mut h = harness(app);
    h.state_mut()
        .open_connection_finder_with("admin@127.0.0.1:1");
    h.run_steps(3);
    h.get_by_label("Connect to admin@127.0.0.1:1");
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert_eq!(h.state().tab_count(), 1);
    assert!(h.state().focused_is_quick_for_test());
}

#[test]
fn the_finder_shortcut_goes_to_the_app_and_not_the_device() {
    let mut h = harness(App::new(location()));
    h.run_steps(2);
    let shortcut = if cfg!(target_os = "macos") {
        egui::Modifiers::MAC_CMD
    } else {
        egui::Modifiers::CTRL | egui::Modifiers::SHIFT
    };
    h.key_press_modifiers(shortcut, egui::Key::K);
    h.run_steps(2);
    assert!(h.state().palette_open_for_test());
    h.key_press_modifiers(shortcut, egui::Key::K);
    h.run_steps(2);
    assert!(!h.state().palette_open_for_test());
}

fn app_shortcut() -> egui::Modifiers {
    if cfg!(target_os = "macos") {
        egui::Modifiers::MAC_CMD
    } else {
        egui::Modifiers::CTRL | egui::Modifiers::SHIFT
    }
}

#[test]
fn a_new_tab_offers_saved_sessions_and_leaves_nothing_if_closed() {
    let mut s = sample();
    s.app.add_demo_tab("core-sw1", Some(s.core_sw), b"BEFORE");
    let mut h = harness(s.app);
    h.run_steps(2);
    h.key_press_modifiers(app_shortcut(), egui::Key::T);
    h.run_steps(3);
    h.get_by_label("New tab");
    assert!(h.state().palette_open_for_test());
    assert_eq!(
        h.state().tab_count(),
        1,
        "not a real tab until something is chosen"
    );
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert!(h.query_by_label("New tab").is_none());
    assert!(!h.state().palette_open_for_test());
    assert_eq!(h.state().tab_count(), 1);
    assert!(h.state().focused_screen_contains("BEFORE"));
}

#[test]
fn choosing_in_a_new_tab_opens_it() {
    let mut s = sample();
    s.app.add_demo_tab("core-sw1", Some(s.core_sw), b"BEFORE");
    s.app
        .config
        .add_session(None, "only-here", target("127.0.0.1"))
        .unwrap();
    let mut h = harness(s.app);
    h.run_steps(2);
    h.key_press_modifiers(app_shortcut(), egui::Key::T);
    h.run_steps(3);
    h.get_all_by_role(egui::accesskit::Role::TextInput)
        .last()
        .unwrap()
        .type_text("only-here");
    h.run_steps(3);
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert_eq!(h.state().tab_count(), 2);
    assert!(!h.state().palette_open_for_test());
}

/// Plain Ctrl+T means something to a device.
#[test]
fn only_the_app_shortcut_opens_a_new_tab() {
    let mut h = harness(App::new(location()));
    h.run_steps(2);
    if !cfg!(target_os = "macos") {
        h.key_press_modifiers(egui::Modifiers::CTRL, egui::Key::T);
        h.run_steps(2);
        assert!(!h.state().palette_open_for_test());
    }
}

#[test]
fn the_menus_run_the_same_commands_as_the_shortcuts() {
    if cfg!(target_os = "macos") {
        return; // native there, and the window has no menu row
    }
    let mut h = harness(App::new(location()));
    h.run_steps(3);
    h.get_by_label("File").click();
    h.run_steps(3);
    h.get_by_label_contains("New Tab").click();
    h.run_steps(3);
    assert!(h.state().palette_open_for_test());
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    h.get_by_label("File").click();
    h.run_steps(3);
    h.get_by_label_contains("Preferences").click();
    h.run_steps(3);
    h.get_by_label("Global settings");
}

#[test]
fn check_for_updates_is_only_on_builds_that_can() {
    if cfg!(target_os = "macos") {
        return;
    }
    let mut h = harness(App::new(location()));
    h.run_steps(3);
    h.get_by_label("File").click();
    h.run_steps(3);
    assert!(h.query_by_label("Check for Updates…").is_none());
    h.key_press(egui::Key::Escape);
    h.state_mut().set_update_key([1; 32]);
    h.state_mut().set_update_checks(Some(false));
    h.run_steps(3);
    h.get_by_label("File").click();
    h.run_steps(3);
    assert!(h.query_by_label("Check for Updates…").is_some());
}

#[test]
fn about_shows_the_version_and_closes() {
    let mut h = harness(App::new(location()));
    h.state_mut().open_about_for_test();
    h.run_steps(3);
    h.get_by_label_contains("Version ");
    h.get_by_label("By Tony Mattke");
    assert!(h.query_by_label("line con 0").is_none());
    h.get_by_label("Close").click();
    h.run_steps(3);
    assert!(h.query_by_label_contains("Version ").is_none());
}

#[test]
fn the_session_manager_lists_searches_and_edits() {
    let mut s = sample();
    let folder = s.app.config.add_folder(None, "Datacenter").unwrap();
    s.app
        .config
        .add_session(Some(folder), "edge-fw", target("172.16.9.9"))
        .unwrap();
    let mut h = harness(s.app);
    h.run_steps(2);
    h.key_press_modifiers(app_shortcut(), egui::Key::O);
    h.run_steps(3);
    assert!(h.state().manager_open_for_test());
    h.get_by_label("New session");
    let boxes =
        |h: &Harness<'static, App>| h.get_all_by_role(egui::accesskit::Role::TextInput).count();
    assert!(boxes(&h) >= 2);
    h.get_by_label("Close").click();
    h.run_steps(3);
    assert!(!h.state().manager_open_for_test());
}

/// Choosing the manager means no sidebar at all, not a hidden one.
#[test]
fn choosing_the_session_manager_removes_the_sidebar_and_stays() {
    use crate::app::Command;
    let place = location();
    let app = App::new(place.clone());
    let mut h = harness(app);
    h.run_steps(3);
    h.get_by_label("linecon0");
    assert!(h.query_by_label("‹").is_some(), "the sidebar starts on");
    let ctx = h.ctx.clone();
    h.state_mut()
        .run_command_for_test(&ctx, Command::ToggleSessionManagerPreference);
    h.run_steps(3);
    assert!(h.state().manager_open_for_test());
    h.get_by_label("Close").click();
    h.run_steps(3);
    // No sidebar, no strip, and Cmd+B cannot bring it back.
    assert!(h.query_by_label("‹").is_none());
    assert!(h.query_by_label("›").is_none());
    h.key_press_modifiers(app_shortcut(), egui::Key::B);
    h.run_steps(3);
    assert!(h.query_by_label("‹").is_none());
    h.state_mut().save_for_test();
    drop(h);
    // Across a restart too. The lock went with the first window.
    let mut h = harness(App::new(place));
    h.run_steps(3);
    assert!(h.query_by_label("‹").is_none());
    assert!(h.query_by_label("›").is_none());
    let ctx = h.ctx.clone();
    h.state_mut()
        .run_command_for_test(&ctx, Command::ToggleSessionManagerPreference);
    h.run_steps(3);
    h.get_by_label("‹");
}

#[test]
fn messages_are_still_seen_with_no_sidebar() {
    let mut s = sample();
    s.app.config.use_session_manager = true;
    s.app.show_info_for_test("Saved the thing");
    let mut h = harness(s.app);
    h.run_steps(3);
    h.get_by_label("Saved the thing");
}

/// While About is up, keys are for it, including the Esc that closes it.
#[test]
fn the_about_box_keeps_keys_from_the_device() {
    use std::io::Read;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 64];
        while let Ok(n) = stream.read(&mut buf) {
            if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    let mut h = harness(App::new(location()));
    h.run_steps(3);
    h.state_mut().quick_open(&format!("raw://127.0.0.1:{port}"));
    let settle = |h: &mut Harness<'static, App>| {
        let until = std::time::Instant::now() + std::time::Duration::from_millis(600);
        while std::time::Instant::now() < until {
            h.step();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    };
    settle(&mut h);
    h.state_mut().open_about_for_test();
    h.run_steps(3);
    h.key_press(egui::Key::A);
    h.key_press(egui::Key::Escape);
    settle(&mut h);
    assert!(
        h.query_by_label_contains("Version ").is_none(),
        "still open"
    );
    let got: Vec<u8> = rx.try_iter().flatten().collect();
    assert!(got.is_empty(), "sent to the device: {got:?}");
}

fn has_keyboard(h: &Harness<'static, App>, label: &str) -> bool {
    use egui_kittest::kittest::NodeT;
    h.get_by_label(label).accesskit_node().is_focused()
}

fn host_key_prompt(known_types: Vec<String>, changed: bool) -> HostKeyPrompt {
    HostKeyPrompt {
        key_type: "ssh-ed25519".into(),
        key: "abc".into(),
        port: 22,
        description: "The fingerprint is:\n  ssh-ed25519 255 SHA256:abcdef".into(),
        known_types,
        changed,
    }
}

/// Enter takes a key the first time a host is seen, and nothing has to be clicked.
#[test]
fn enter_trusts_a_new_host() {
    let mut s = sample();
    s.app.add_demo_tab("core-sw1", Some(s.core_sw), b"");
    s.app
        .demo_pending(Pending::HostKey(host_key_prompt(Vec::new(), false)));
    let mut h = harness(s.app);
    h.run_steps(3);
    assert!(has_keyboard(&h, "Trust and remember"));
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert!(h.query_by_label("Trust and remember").is_none());
}

/// A contradicted key must never be one keystroke from being trusted.
#[test]
fn enter_never_trusts_a_changed_key() {
    for (known, changed) in [(Vec::new(), true), (vec!["ssh-rsa".to_string()], false)] {
        let mut s = sample();
        s.app.add_demo_tab("core-sw1", Some(s.core_sw), b"");
        s.app
            .demo_pending(Pending::HostKey(host_key_prompt(known, changed)));
        let mut h = harness(s.app);
        h.run_steps(3);
        assert!(has_keyboard(&h, "Cancel"));
        assert!(!has_keyboard(&h, "Trust and remember"));
    }
}

/// Typing in a field stays in that field when a question arrives.
#[test]
fn a_host_key_question_does_not_take_a_field_from_the_user() {
    let mut s = sample();
    s.app.add_demo_tab("core-sw1", Some(s.core_sw), b"");
    let mut h = harness(s.app);
    h.run_steps(3);
    let field = egui::Id::unique("some-field");
    h.ctx.memory_mut(|m| m.request_focus(field));
    h.state_mut()
        .demo_pending(Pending::HostKey(host_key_prompt(Vec::new(), false)));
    h.run_steps(1);
    assert!(!has_keyboard(&h, "Trust and remember"));
}

#[test]
fn only_a_host_with_nothing_remembered_counts_as_new() {
    assert!(host_key_prompt(Vec::new(), false).is_new_host());
    assert!(!host_key_prompt(Vec::new(), true).is_new_host());
    assert!(!host_key_prompt(vec!["ssh-rsa".into()], false).is_new_host());
    assert_eq!(
        host_key_prompt(Vec::new(), false).fingerprint(),
        "ssh-ed25519 255 SHA256:abcdef"
    );
}

#[test]
fn the_session_manager_opens_with_the_keyboard_in_its_search() {
    let mut h = harness(App::new(location()));
    h.run_steps(2);
    h.key_press_modifiers(app_shortcut(), egui::Key::O);
    h.run_steps(3);
    assert_eq!(
        h.ctx.memory(|m| m.focused()),
        Some(egui::Id::unique("linecon-manager-search"))
    );
}

/// On real SSH: a new host connects without asking and is remembered, and a contradicted key
/// still stops and asks, with the setting on.
#[test]
fn the_trust_setting_only_covers_new_hosts() {
    use crate::pane::{Pane, PaneSpec, Status};
    use linecon_engine::hostkeys::{HostKeyLookup, HostKeyStore, MemoryHostKeys};
    use linecon_engine::HostKeyVerdict;
    use std::process::Command;
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use zeroize::Zeroizing;

    /// Remembers keys, and claims anything at this address has changed.
    #[derive(Default)]
    struct Store(MemoryHostKeys);
    impl HostKeyStore for Store {
        fn check(&self, host: &str, port: i32, key_type: &str, key: &str) -> HostKeyLookup {
            if host == "127.0.0.2" {
                HostKeyLookup::Changed
            } else {
                self.0.check(host, port, key_type, key)
            }
        }
        fn store(&self, host: &str, port: i32, key_type: &str, key: &str) {
            self.0.store(host, port, key_type, key);
        }
    }

    let docker = |args: &[&str]| -> Option<String> {
        let out = Command::new("docker").args(args).output().ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let context = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/docker/legacy-sshd"
    );
    let name = format!("linecon-trust-{}", std::process::id());
    let started = docker(&["build", "-q", "-t", "linecon-legacy-sshd", context]).and_then(|_| {
        docker(&[
            "run",
            "-d",
            "--rm",
            "--name",
            &name,
            "-P",
            "linecon-legacy-sshd",
        ])
    });
    if started.is_none() {
        assert!(
            std::env::var_os("LINECON_REQUIRE_DOCKER").is_none(),
            "Docker is required here"
        );
        eprintln!("SKIPPED: no container runtime");
        return;
    }
    struct Stop<'a>(&'a str);
    impl Drop for Stop<'_> {
        fn drop(&mut self) {
            let _ = Command::new("docker").args(["rm", "-f", self.0]).output();
        }
    }
    let _stop = Stop(&name);
    let port: u16 = docker(&["port", &name, "2201"])
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .rsplit(':')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    std::thread::sleep(Duration::from_secs(1));

    let store = Arc::new(Store::default());
    linecon_engine::set_hostkey_store(store.clone());
    let run = |host: &str| {
        let mut settings = linecon_engine::settings::Settings::ssh(host, port, "lab");
        settings.algorithms(&AlgorithmPolicy::legacy());
        Pane::new(
            PaneSpec {
                name: "lab".into(),
                session_id: None,
                settings,
                policy: AlgorithmPolicy::legacy(),
                log_dir: None,
                scrollback: 1000,
                copy_on_select: true,
                auto_reconnect: false,
                accept_weak: true,
                trust_new_host_keys: true,
                password: Some(Arc::new(|| {
                    Some(Zeroizing::new("lab-password".to_string()))
                })),
            },
            Arc::new(|| {}),
        )
    };

    let pane = run("127.0.0.1");
    let deadline = Instant::now() + Duration::from_secs(30);
    while pane.status() != Status::Live {
        assert!(Instant::now() < deadline, "{:?}", pane.status());
        assert!(
            pane.shared.pending.lock().unwrap().is_none(),
            "a new host was asked about"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let text = pane.shared.core.lock().unwrap().text_with_scrollback(500);
    assert!(text.contains("remembered this new host's key"), "{text}");
    assert!(text.contains("SHA256:"), "{text}");
    assert!(!store.0.known_types("127.0.0.1", port as i32).is_empty());
    drop(pane);

    let pane = run("127.0.0.2");
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        assert!(
            Instant::now() < deadline,
            "never asked about a changed key: {:?}",
            pane.status()
        );
        assert_ne!(pane.status(), Status::Live, "connected past a changed key");
        let pending = pane.shared.pending.lock().unwrap().clone();
        if let Some(Pending::HostKey(key)) = pending {
            assert!(key.changed);
            pane.answer_hostkey(HostKeyVerdict::Reject);
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(store.0.known_types("127.0.0.2", port as i32).is_empty());
}

/// Shortcuts come from the menus, so the one in a menu is the one that runs.
#[test]
fn the_session_settings_shortcut_is_in_its_menu_and_works() {
    let mut s = sample();
    s.app.add_demo_tab("core-sw1", Some(s.core_sw), b"");
    let mut h = harness(s.app);
    h.run_steps(3);
    // macOS shows its menus outside the window.
    if !cfg!(target_os = "macos") {
        h.get_by_label("View").click();
        h.run_steps(3);
        h.get_by_label_contains("Session Settings");
        h.key_press(egui::Key::Escape);
        h.run_steps(3);
    }
    assert!(h.query_by_label("Connect").is_none());
    h.key_press_modifiers(app_shortcut(), egui::Key::I);
    h.run_steps(3);
    h.get_by_label("Connect");
}

#[test]
fn the_first_screen_is_captured_for_review() {
    let mut h = harness(App::new(location()));
    h.run_steps(3);
    screenshot(&mut h, "first-screen");
}

/// A field holding the keyboard, with a session open, once redrew flat out and took a whole
/// core for as long as it stayed there.
#[test]
fn a_focused_field_does_not_keep_the_window_redrawing() {
    let mut s = sample();
    s.app.add_demo_tab("core-sw1", Some(s.core_sw), b"");
    let mut h = harness(s.app);
    h.run_steps(3);
    h.get_all_by_role(egui::accesskit::Role::TextInput)
        .next()
        .unwrap()
        .click();
    h.run_steps(5);
    assert!(h.ctx.memory(|m| m.focused()).is_some());
    h.run_steps(5);
    assert!(h.run_ok().is_some(), "never stopped redrawing");
}

/// Typing in a field writes once, after a pause.
#[test]
fn changes_save_after_a_pause() {
    let place = location();
    let mut h = harness(App::new(place.clone()));
    h.run_steps(3);
    h.state_mut().set_update_checks(Some(false));
    h.state_mut().mark_dirty_for_test();
    h.run_steps(2);
    let saved = |place: &linecon_config::ConfigLocation| {
        std::fs::read_to_string(place.config_file()).unwrap_or_default()
    };
    assert!(!saved(&place).contains("update_checks"), "saved at once");
    std::thread::sleep(std::time::Duration::from_millis(700));
    h.run_steps(2);
    // The write itself happens on the saver's thread.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !saved(&place).contains("update_checks") {
        assert!(std::time::Instant::now() < deadline, "never saved");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[test]
fn saving_on_exit_waits_for_the_write() {
    let place = location();
    let mut app = App::new(place.clone());
    app.set_update_checks(Some(false));
    app.mark_dirty_for_test();
    let started = std::time::Instant::now();
    let mut h = harness(app);
    std::thread::sleep(std::time::Duration::from_millis(700));
    h.run_steps(2);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "the window waited"
    );
    // Saving on exit still waits, so nothing is lost.
    h.state_mut().save_before_exit();
    let text = std::fs::read_to_string(place.config_file()).unwrap();
    assert!(text.contains("update_checks"));
}

fn open_new_session(h: &mut Harness<'static, App>) {
    h.get_by_label("+").click();
    h.run_steps(3);
    h.get_by_label("New session").click();
    h.run_steps(4);
}

fn session_named(app: &App, name: &str) -> Option<Id> {
    fn walk(nodes: &[linecon_config::Node], name: &str) -> Option<Id> {
        nodes.iter().find_map(|node| match node {
            linecon_config::Node::Session(s) if s.name == name => Some(s.id),
            linecon_config::Node::Folder(f) => walk(&f.children, name),
            _ => None,
        })
    }
    walk(&app.config.nodes, name)
}

/// The keyboard starts in the name, and Tab goes name, protocol, host, without leaving.
#[test]
fn a_new_session_starts_in_its_name_and_tabs_stay_in_the_dialog() {
    let mut s = sample();
    s.app.add_demo_tab("core-sw1", Some(s.core_sw), b"");
    let mut h = harness(s.app);
    h.run_steps(3);
    open_new_session(&mut h);
    h.event(egui::Event::Text("lab-router".into()));
    h.run_steps(2);
    assert!(h.query_all_by_value("lab-router").next().is_some());
    let host_next = |h: &mut Harness<'static, App>| {
        h.key_press(egui::Key::Tab);
        h.run_steps(2);
    };
    host_next(&mut h); // protocol
    assert!(has_keyboard(&h, "SSH"));
    h.key_press(egui::Key::ArrowRight);
    h.run_steps(3);
    assert!(has_keyboard(&h, "Telnet"), "arrows change the protocol");
    h.key_press(egui::Key::ArrowLeft);
    h.run_steps(3);
    assert!(has_keyboard(&h, "SSH"));
    host_next(&mut h); // host
    h.event(egui::Event::Text("10.0.0.9".into()));
    h.run_steps(2);
    assert!(h.query_all_by_value("10.0.0.9").next().is_some());
    for _ in 0..12 {
        host_next(&mut h);
        assert!(!has_keyboard(&h, "core-sw1"), "Tab left the dialog");
    }
}

/// Nothing exists until it is saved, and a selected session does not decide where it goes.
#[test]
fn a_new_session_saves_at_the_top_level_and_cancel_leaves_nothing() {
    let mut s = sample();
    s.app.selected_node_for_test(s.core_sw);
    let before = s.app.config.nodes.len();
    let mut h = harness(s.app);
    h.run_steps(3);
    open_new_session(&mut h);
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert_eq!(h.state().config.nodes.len(), before);
    assert!(h.query_by_label("Save").is_none());

    open_new_session(&mut h);
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert!(h.query_by_label("Save").is_some(), "saved with no host");

    // Enter moved to the host, the part that was missing, and the name defaults to it.
    h.event(egui::Event::Text("10.0.0.9".into()));
    h.run_steps(2);
    h.get_by_label("Save").click();
    h.run_steps(3);
    assert!(h.query_by_label("Save").is_none());
    assert_eq!(h.state().config.nodes.len(), before + 1);
    let id = session_named(h.state(), "10.0.0.9").expect("saved");
    assert!(h.state().config.nodes.iter().any(|node| node.id() == id));
    assert!(h.query_all_by_label("10.0.0.9").next().is_some());
    h.get_by_label_contains("Saved 10.0.0.9 to the top level.");
}

/// A selected folder is where it goes, and the folder opens so the result can be seen.
#[test]
fn a_session_made_in_a_collapsed_folder_is_shown() {
    let mut s = sample();
    let folder = s.app.config.add_folder(None, "Lab").unwrap();
    if let Some(linecon_config::Node::Folder(f)) = s.app.config.find_mut(folder) {
        f.collapsed = true;
    }
    s.app.selected_node_for_test(folder);
    s.app.start_new_session_in_selection_for_test();
    let mut h = harness(s.app);
    h.run_steps(4);
    h.get_by_label_contains("Lab");
    h.event(egui::Event::Text("inside".into()));
    h.run_steps(2);
    h.key_press(egui::Key::Tab);
    h.run_steps(2);
    h.key_press(egui::Key::Tab);
    h.run_steps(2);
    h.event(egui::Event::Text("10.0.0.9".into()));
    h.run_steps(2);
    screenshot(&mut h, "new-session");
    h.key_press(egui::Key::Enter);
    h.run_steps(4);
    screenshot(&mut h, "new-session-saved");
    h.get_by_label("inside");
    let id = session_named(h.state(), "inside").unwrap();
    let path = h.state().config.ancestors(id).unwrap();
    assert_eq!(path.last().unwrap().id, folder);
}

/// Each part of the settings is reachable, and the panel opens on the short one.
#[test]
fn settings_are_split_into_sections() {
    let mut s = sample();
    let id = s.core_sw;
    s.app.inspect(id);
    let mut h = harness(s.app);
    h.run_steps(3);
    h.get_by_label("Protocol");
    h.get_by_label("Changes are saved");
    assert!(h.query_by_label("Theme").is_none());
    h.get_by_label("Terminal").click();
    h.run_steps(3);
    h.get_by_label("Theme");
    h.get_by_label("Logging").click();
    h.run_steps(3);
    h.get_by_label("Log folder");
    h.get_by_label("SSH").click();
    h.run_steps(3);
    h.get_by_label("Jump host");
    assert!(h.query_by_label("Serial").is_none(), "this one is SSH");
    screenshot(&mut h, "settings-ssh");
}

#[test]
fn only_a_selected_folder_decides_where_new_things_go() {
    let mut s = sample();
    let folder = s.app.config.add_folder(None, "Lab").unwrap();
    assert_eq!(s.app.selected_folder_for_test(), None);
    s.app.selected_node_for_test(s.core_sw);
    assert_eq!(
        s.app.selected_folder_for_test(),
        None,
        "a session is no folder"
    );
    s.app.selected_node_for_test(folder);
    assert_eq!(s.app.selected_folder_for_test(), Some(folder));
}

/// The first key after closing a field belongs to the device. Losing it turned `exit` into
/// `xit` on a real switch. That needed a window idling between frames, so the window checks
/// are what catch that case: here the harness always draws the frame that hid it.
#[test]
fn the_first_key_after_a_field_closes_reaches_the_device() {
    use std::io::Read;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        tx.send(Vec::new()).unwrap();
        let mut buf = [0u8; 64];
        while let Ok(n) = stream.read(&mut buf) {
            if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    let mut h = harness(App::new(location()));
    h.run_steps(3);
    h.state_mut().quick_open(&format!("raw://127.0.0.1:{port}"));
    // A busy machine can take a while to connect, and typing before then proves nothing.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while rx.try_recv().is_err() {
        assert!(std::time::Instant::now() < deadline, "never connected");
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let until = std::time::Instant::now() + std::time::Duration::from_millis(500);
    while std::time::Instant::now() < until {
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let ctx = h.ctx.clone();
    h.state_mut()
        .run_command_for_test(&ctx, crate::app::Command::GlobalSettings);
    h.run_steps(3);
    h.get_all_by_role(egui::accesskit::Role::TextInput)
        .last()
        .unwrap()
        .click();
    h.run_steps(3);
    assert!(h.ctx.memory(|m| m.focused()).is_some(), "no field has it");
    h.get_all_by_label("×").next().unwrap().click();
    assert!(h.run_ok().is_some());
    assert!(h.query_by_label("Global settings").is_none(), "still open");
    h.event(egui::Event::Text("e".into()));
    h.step();
    let got = rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("the key never arrived");
    assert_eq!(got, b"e");
}

/// Deleting asks in a dialog, and Enter or Escape keep what was there.
#[test]
fn deleting_asks_and_the_keyboard_keeps_it() {
    let mut s = sample();
    let first = s.app.config.nodes[0].id();
    let count = s.app.config.nodes.len();
    s.app.ask_delete_first_folder();
    let mut h = harness(s.app);
    h.run_steps(4);
    h.get_by_label_contains("and everything in it?");
    assert!(has_keyboard(&h, "Keep"));
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert_eq!(h.state().config.nodes.len(), count);
    assert!(h.query_by_label_contains("everything in it?").is_none());

    h.state_mut().ask_delete_first_folder();
    h.run_steps(4);
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert_eq!(h.state().config.nodes.len(), count);
    assert!(h.query_by_label_contains("everything in it?").is_none());

    h.state_mut().ask_delete_first_folder();
    h.run_steps(4);
    screenshot(&mut h, "confirm-delete");
    h.get_by_label("Delete").click();
    h.run_steps(3);
    assert_eq!(h.state().config.nodes.len(), count - 1);
    assert!(h.state().config.find(first).is_none());
}

/// Opening a session goes to it, however it was opened from the manager.
#[test]
fn opening_from_the_session_manager_closes_it_and_shows_the_session() {
    let s = sample();
    let mut h = harness(s.app);
    h.run_steps(2);
    h.key_press_modifiers(app_shortcut(), egui::Key::O);
    h.run_steps(3);
    assert!(h.state().manager_open_for_test());
    // The search has the keyboard: type, take the second result with the arrow, press Enter.
    h.event(egui::Event::Text("core-sw".into()));
    h.run_steps(3);
    h.key_press(egui::Key::ArrowDown);
    h.run_steps(3);
    h.key_press(egui::Key::Enter);
    h.run_steps(4);
    assert!(!h.state().manager_open_for_test(), "still open");
    assert_eq!(h.state().tab_count_for_test(), 1);
    assert_eq!(h.state().tab_title_for_test().as_deref(), Some("core-sw2"));

    // Clicking a result does the same.
    h.key_press_modifiers(app_shortcut(), egui::Key::O);
    h.run_steps(3);
    assert!(h.state().manager_open_for_test());
    h.event(egui::Event::Text("edge-rtr1".into()));
    h.run_steps(3);
    h.get_all_by_label("edge-rtr1").last().unwrap().click();
    h.run_steps(4);
    assert!(!h.state().manager_open_for_test(), "still open");
    assert_eq!(h.state().tab_count_for_test(), 2);
}

#[test]
fn the_window_comes_back_as_it_was_left_without_connecting() {
    let place = location();
    let (sw1, sw2);
    {
        let mut app = App::new(place.clone());
        let lab = app.config.add_folder(None, "Lab").unwrap();
        sw1 = app
            .config
            .add_session(Some(lab), "lab-sw1", target("127.0.0.1"))
            .unwrap();
        sw2 = app
            .config
            .add_session(Some(lab), "lab-sw2", target("127.0.0.1"))
            .unwrap();
        app.mark_dirty_for_test();
        app.open_saved_for_test(sw1);
        app.open_saved_for_test(sw2);
        app.open_saved_for_test(sw1);
        app.selected_node_for_test(sw2);
        app.hide_sidebar_for_test();
        app.save_before_exit();
    }
    let app = App::new(place);
    assert_eq!(app.tab_count_for_test(), 0, "nothing reconnects on its own");
    assert_eq!(app.selected_for_test(), Some(sw2));
    assert!(!app.sidebar_open_for_test());
    let mut h = harness(app);
    h.run();
    h.get_by_label("Open lab-sw1 and lab-sw2 again").click();
    h.run();
    h.run();
    assert_eq!(h.state().tab_count_for_test(), 2, "one tab per session");
    assert!(
        h.query_by_label("Open lab-sw1 and lab-sw2 again").is_none(),
        "the offer goes once taken up"
    );
}

#[test]
fn a_damaged_window_state_is_ignored() {
    let place = location();
    std::fs::create_dir_all(&place.dir).unwrap();
    std::fs::write(place.dir.join("window-state.json"), b"{ not json").unwrap();
    let app = App::new(place);
    assert!(app.sidebar_open_for_test());
    assert_eq!(app.tab_count_for_test(), 0);
}

#[test]
fn a_tabs_close_button_closes_all_of_its_splits() {
    let mut s = sample();
    s.app
        .add_demo_tab("core-sw1", Some(s.core_sw), SHOW_VERSION);
    s.app.split_demo(TOP);
    s.app.add_demo_tab("edge-rtr1", None, TOP);
    s.app.set_active_tab_for_test(0);
    assert_eq!(s.app.pane_count(), 3);
    let mut h = harness(s.app);
    h.run_steps(3);
    h.get_all_by_label("×")
        .next()
        .expect("a close button")
        .click();
    h.run_steps(3);
    assert_eq!(h.state().tab_count(), 1);
    assert_eq!(
        h.state().pane_count(),
        1,
        "both halves of the split are gone"
    );
    assert_eq!(h.state().tab_title_for_test().as_deref(), Some("edge-rtr1"));
}

#[test]
fn a_split_inside_a_split_can_be_dragged_on_its_own() {
    let mut s = sample();
    s.app
        .add_demo_tab("core-sw1", Some(s.core_sw), SHOW_VERSION);
    // Both split side by side from the same pane, so they start at the same corner.
    s.app.split_demo(TOP);
    s.app.split_demo(TOP);
    let mut h = harness(s.app);
    h.run_steps(3);
    // Each pass adds its dividers; the last pass's two are what is on screen.
    let handles = |h: &Harness<'_, App>| {
        let all = h.ctx.data_mut(|d| {
            std::mem::take(
                d.get_temp_mut_or_default::<Vec<egui::Rect>>(egui::Id::unique("split-handles")),
            )
        });
        all[all.len().saturating_sub(2)..].to_vec()
    };
    h.run();
    let found = handles(&h);
    assert_eq!(found.len(), 2, "{found:?}");
    let (inner, outer) = if found[0].center().x < found[1].center().x {
        (found[0], found[1])
    } else {
        (found[1], found[0])
    };
    let from = inner.center();
    h.drag_at(from);
    h.run();
    h.hover_at(from - egui::vec2(60.0, 0.0));
    h.run();
    h.drop_at(from - egui::vec2(60.0, 0.0));
    h.run();
    handles(&h);
    h.run();
    let after = handles(&h);
    let moved = after
        .iter()
        .any(|r| (r.center().x - (from.x - 60.0)).abs() < 2.0);
    assert!(moved, "the inner divider did not follow: {after:?}");
    assert!(
        after
            .iter()
            .any(|r| (r.center().x - outer.center().x).abs() < 0.5),
        "the outer divider moved too: {after:?}"
    );
}

#[test]
fn alt_and_a_letter_reaches_the_peer_once() {
    use std::io::{Read, Write};
    if cfg!(target_os = "macos") {
        // Option types characters there, and is not sent as Escape.
        return;
    }
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
        assert!(std::time::Instant::now() < deadline, "never connected");
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    h.step();
    // What egui-winit hands over for Alt+B: the key, then the letter as text.
    h.event(egui::Event::Key {
        key: egui::Key::B,
        physical_key: Some(egui::Key::B),
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::ALT,
    });
    h.event(egui::Event::Text("b".into()));
    h.event(egui::Event::Text("x".into()));
    h.step();
    let mut seen = Vec::new();
    while seen.len() < 3 {
        seen.extend(
            rx.recv_timeout(std::time::Duration::from_secs(20))
                .expect("peer got input"),
        );
    }
    std::thread::sleep(std::time::Duration::from_millis(200));
    while let Ok(more) = rx.try_recv() {
        seen.extend(more);
    }
    assert_eq!(
        seen,
        [0x1b, b'b', b'x'],
        "{:?}",
        String::from_utf8_lossy(&seen)
    );
}

#[test]
fn unlocking_saved_passwords_keeps_the_window_drawing() {
    let place = location();
    // A vault file made earlier with a known passphrase.
    std::fs::create_dir_all(&place.dir).unwrap();
    let mut made = linecon_config::secrets::EncryptedFile::open(&place.secrets_file(), "right")
        .expect("create");
    use linecon_config::secrets::SecretStore;
    made.set("probe", "value").unwrap();
    drop(made);

    let mut app = App::new(place);
    assert!(app.vault_locked_for_test(), "portable mode starts locked");
    app.open_unlock_for_test();
    let mut h = harness(app);
    h.run_steps(2);

    h.get_by_role(egui::accesskit::Role::PasswordInput)
        .type_text("wrong");
    h.run_steps(1);
    h.get_by_label("Unlock").click();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while h.state().unlock_error_for_test().is_none() {
        assert!(std::time::Instant::now() < deadline, "no error shown");
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(h.state().vault_locked_for_test());

    h.get_by_role(egui::accesskit::Role::PasswordInput)
        .type_text("right");
    h.run_steps(1);
    h.get_by_label("Unlock").click();
    h.step();
    let mut saw_working = false;
    while h.state().vault_locked_for_test() {
        assert!(std::time::Instant::now() < deadline, "never unlocked");
        saw_working |= h.query_by_label("Unlocking…").is_some();
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(
        saw_working,
        "the window did not draw while the key was derived"
    );
    h.run_steps(2);
    assert!(h.query_by_label("Unlock saved passwords").is_none());
}

#[test]
fn the_session_tree_works_from_the_keyboard() {
    let s = sample();
    let core_sw2 = session_named(&s.app, "core-sw2").unwrap();
    let edge_rtr1 = session_named(&s.app, "edge-rtr1").unwrap();
    let mut h = harness(s.app);
    h.run_steps(3);
    h.get_by_label("core-sw1").click();
    h.run_steps(2);
    let press = |h: &mut Harness<'static, App>, key| {
        h.key_press(key);
        h.run_steps(3);
    };
    let selected = |h: &Harness<'static, App>| {
        let id = h.state().selected_for_test()?;
        let name = h.state().config_for_test().find(id)?.name().to_string();
        Some(name)
    };

    press(&mut h, egui::Key::ArrowDown);
    assert_eq!(h.state().selected_for_test(), Some(core_sw2));
    assert!(has_keyboard(&h, "core-sw2"), "the keyboard follows");
    press(&mut h, egui::Key::ArrowDown);
    assert_eq!(selected(&h).as_deref(), Some("Edge"));
    // Left closes an open folder, then goes up to the one holding it.
    press(&mut h, egui::Key::ArrowLeft);
    assert!(
        h.query_by_label("edge-rtr1").is_none(),
        "Edge did not close"
    );
    press(&mut h, egui::Key::ArrowLeft);
    assert_eq!(selected(&h).as_deref(), Some("Datacenter"));
    // Right on Edge opens it, and again steps inside.
    press(&mut h, egui::Key::End);
    press(&mut h, egui::Key::Home);
    assert_eq!(selected(&h).as_deref(), Some("Datacenter"));
    for _ in 0..4 {
        press(&mut h, egui::Key::ArrowDown);
    }
    assert_eq!(selected(&h).as_deref(), Some("Edge"));
    press(&mut h, egui::Key::ArrowRight);
    press(&mut h, egui::Key::ArrowRight);
    assert_eq!(h.state().selected_for_test(), Some(edge_rtr1));
    // Enter connects, F2 renames, Delete asks.
    press(&mut h, egui::Key::F2);
    assert!(h.state().renaming_for_test());
    press(&mut h, egui::Key::Escape);
    h.get_by_label("edge-rtr1").click();
    h.run_steps(2);
    press(&mut h, egui::Key::Delete);
    h.get_by_label("Delete edge-rtr1?");
    press(&mut h, egui::Key::Escape);
    assert!(
        h.state().config_for_test().find(edge_rtr1).is_some(),
        "Escape keeps it"
    );
    h.get_by_label("edge-rtr1").click();
    h.run_steps(2);
    press(&mut h, egui::Key::Enter);
    assert_eq!(h.state().tab_count_for_test(), 1, "Enter connects");
}

#[test]
fn several_sessions_move_and_delete_together() {
    let s = sample();
    let sw1 = s.core_sw;
    let sw2 = session_named(&s.app, "core-sw2").unwrap();
    let rtr = session_named(&s.app, "edge-rtr1").unwrap();
    let mut h = harness(s.app);
    h.run_steps(3);
    let parent_name = |h: &Harness<'static, App>, id| {
        let path = h.state().config_for_test().ancestors(id)?;
        Some(path.last()?.name.clone())
    };

    // Ctrl (Command on a Mac) adds to the selection, and a drag takes both.
    h.get_by_label("core-sw1").click();
    h.run_steps(2);
    h.get_by_label("edge-rtr1")
        .click_modifiers(egui::Modifiers::COMMAND);
    h.run_steps(2);
    let from = h.get_by_label("edge-rtr1").rect().center();
    let to = h.get_by_label("Branch office").rect().center();
    h.drag_at(from);
    h.run();
    h.hover_at(to);
    h.run();
    h.drop_at(to);
    h.run_steps(3);
    assert_eq!(parent_name(&h, sw1).as_deref(), Some("Branch office"));
    assert_eq!(parent_name(&h, rtr).as_deref(), Some("Branch office"));
    assert_eq!(parent_name(&h, sw2).as_deref(), Some("Core switches"));

    // Shift takes a range, and one question covers it. The two moved here are last.
    h.get_by_label("br-sw1").click();
    h.run_steps(2);
    h.get_by_label("edge-rtr1")
        .click_modifiers(egui::Modifiers::SHIFT);
    h.run_steps(2);
    h.key_press(egui::Key::Delete);
    h.run_steps(3);
    h.get_by_label("Delete these 4 sessions?");
    h.get_by_label("Delete").click();
    h.run_steps(3);
    let config = h.state().config_for_test();
    for gone in [sw1, rtr] {
        assert!(config.find(gone).is_none());
    }
    assert!(config.find(sw2).is_some(), "outside the range");
}

#[test]
fn a_proposal_can_be_changed_before_it_is_sent_and_every_call_is_logged() {
    use serde_json::json;
    let (mut h, host, id) = live_agent_pane();
    agent_call(
        host.as_ref(),
        "send_text",
        json!({ "session": id, "text": "reload\n" }),
    );
    agent_call(host.as_ref(), "read_screen", json!({ "session": id }));
    h.run_steps(3);
    h.get_by_label("Edit text").click();
    h.run_steps(2);
    let field = h.get_by_role(egui::accesskit::Role::MultilineTextInput);
    field.focus();
    h.run_steps(1);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.run_steps(1);
    h.get_by_role(egui::accesskit::Role::MultilineTextInput)
        .type_text("show clock\n");
    h.run_steps(2);
    // Typing in the field must not approve it.
    assert_eq!(
        agent_call(host.as_ref(), "get_proposal", json!({ "proposal": 1 })).1,
        "pending"
    );
    h.get_by_label("Send edited").click();
    h.run_steps(3);
    assert_eq!(
        agent_call(host.as_ref(), "get_proposal", json!({ "proposal": 1 })).1,
        "sent_edited"
    );

    let log = std::fs::read_to_string(h.state().agent_log_for_test()).unwrap();
    let entries: Vec<serde_json::Value> = log
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let events: Vec<&str> = entries
        .iter()
        .map(|e| e["event"].as_str().unwrap())
        .collect();
    for wanted in [
        "access_on",
        "list_sessions",
        "propose",
        "read_screen",
        "answer",
    ] {
        assert!(events.contains(&wanted), "{wanted} missing from {events:?}");
    }
    let answer = entries.iter().find(|e| e["event"] == "answer").unwrap();
    assert_eq!(answer["answer"], "sent_edited");
    assert_eq!(answer["text"], "show clock\n");
    let proposed = entries.iter().find(|e| e["event"] == "propose").unwrap();
    assert_eq!(proposed["text"], "reload\n");
    assert!(entries.iter().all(|e| e["time"].is_string()));
    // What the agent read is not copied into the log, only that it read.
    let read = entries
        .iter()
        .find(|e| e["event"] == "read_screen")
        .unwrap();
    assert_eq!(read["session"], id);
    assert!(read.get("text").is_none(), "{read}");
}

/// Every control a screen reader can land on says what it is. Walks the accessibility tree
/// of the main screens and fails on any button, field or choice with no name.
#[test]
fn every_control_has_a_name_for_screen_readers() {
    use egui::accesskit::Role;
    use egui_kittest::kittest::By;
    fn unnamed(h: &Harness<'static, App>, screen: &str) -> Vec<String> {
        let interactive = [
            Role::Button,
            Role::CheckBox,
            Role::RadioButton,
            Role::TextInput,
            Role::MultilineTextInput,
            Role::PasswordInput,
            Role::ComboBox,
            Role::Slider,
            Role::SpinButton,
            Role::Link,
            Role::Tab,
        ];
        h.query_all(By::new().predicate(move |node| {
            interactive.contains(&node.role())
                && node.label().is_none_or(|l| l.trim().is_empty())
                && node.value().is_none_or(|v| v.trim().is_empty())
        }))
        .map(|node| {
            use egui_kittest::kittest::NodeT;
            let n = node.accesskit_node();
            format!("{screen}: {:?} at {:?}", n.role(), n.bounding_box())
        })
        .collect()
    }
    let s = sample();
    let core_sw = s.core_sw;
    let mut app = s.app;
    app.add_demo_tab("core-sw1", Some(core_sw), SHOW_VERSION);
    let mut h = harness(app);
    h.run_steps(3);
    let mut found = unnamed(&h, "main window");

    h.state_mut().inspect(core_sw);
    h.run_steps(3);
    found.extend(unnamed(&h, "session settings"));
    for section in ["Terminal", "Logging", "SSH"] {
        if let Some(tab) = h.query_by_label(section) {
            tab.click();
            h.run_steps(3);
            found.extend(unnamed(&h, section));
        }
    }

    h.state_mut().open_about_for_test();
    h.run_steps(3);
    found.extend(unnamed(&h, "about"));
    h.key_press(egui::Key::Escape);
    h.run_steps(3);

    h.state_mut().start_new_session_in_selection_for_test();
    h.run_steps(3);
    found.extend(unnamed(&h, "new session"));
    h.key_press(egui::Key::Escape);
    h.run_steps(3);

    h.state_mut().open_manager_for_test();
    h.run_steps(4);
    found.extend(unnamed(&h, "session manager"));
    assert!(
        found.is_empty(),
        "controls with no name:\n{}",
        found.join("\n")
    );
}

#[test]
fn highlighting_is_a_terminal_setting_that_applies_at_once() {
    let s = sample();
    let core_sw = s.core_sw;
    let mut app = s.app;
    app.add_demo_tab(
        "core-sw1",
        Some(core_sw),
        b"GigabitEthernet1/0/1 is up, line protocol is up\r\n\
          GigabitEthernet1/0/2 is administratively down, line protocol is down\r\n\
          Internet address is 10.20.0.2/24, MAC 0011.2233.4455\r\n",
    );
    app.inspect(core_sw);
    let mut h = harness(app);
    h.run_steps(3);
    h.get_by_label("Terminal").click();
    h.run_steps(3);
    let resolved = |h: &Harness<'static, App>| {
        h.state()
            .config_for_test()
            .resolve(core_sw)
            .unwrap()
            .highlight_keywords
            .value
    };
    assert!(!resolved(&h), "off unless asked for");
    h.get_by_label("Highlight states and addresses").click();
    h.run_steps(3);
    assert!(resolved(&h));
    screenshot(&mut h, "highlighting");
}

#[test]
fn quick_ctrl_insert_copies_and_ctrl_v_with_a_picture_still_reaches_the_peer() {
    use std::io::{Read, Write};
    if cfg!(target_os = "macos") {
        // Command copies and pastes there; neither ever reaches the device.
        return;
    }
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
        assert!(std::time::Instant::now() < deadline, "never connected");
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    h.step();
    // Ctrl as egui-winit reports it off macOS: also the command key.
    let ctrl = egui::Modifiers::CTRL | egui::Modifiers::COMMAND;
    // A fast Ctrl+Insert: the copy and the release of Insert land in one frame.
    let input = h.input_mut();
    input.events.push(egui::Event::ModifiersChanged(ctrl));
    input.events.push(egui::Event::Copy);
    input.events.push(egui::Event::Key {
        key: egui::Key::Insert,
        physical_key: Some(egui::Key::Insert),
        pressed: false,
        repeat: false,
        modifiers: ctrl,
    });
    h.step();
    // Ctrl+V with only a picture on the clipboard.
    let input = h.input_mut();
    input.events.push(egui::Event::ModifiersChanged(ctrl));
    input
        .events
        .push(egui::Event::PasteImage(std::sync::Arc::new(
            egui::ColorImage::filled([1, 1], egui::Color32::BLACK),
        )));
    h.step();
    h.input_mut()
        .events
        .push(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
    h.step();
    h.event(egui::Event::Text("x".into()));
    h.step();
    let mut seen = Vec::new();
    while !seen.ends_with(b"x") {
        seen.extend(
            rx.recv_timeout(std::time::Duration::from_secs(20))
                .expect("peer got input"),
        );
    }
    assert_eq!(seen, [0x16, b'x'], "{:?}", String::from_utf8_lossy(&seen));
}
