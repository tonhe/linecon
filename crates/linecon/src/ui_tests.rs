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
