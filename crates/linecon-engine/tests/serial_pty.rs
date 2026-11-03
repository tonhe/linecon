//! The serial backend against a pseudo-terminal. Unix only: Windows has no
//! equivalent without a virtual port driver, so it gets build and link coverage.
#![cfg(unix)]

use std::ffi::CStr;
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::FromRawFd;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use linecon_engine::settings::Settings;
use linecon_engine::{Event, OpenError, RecvError, Session, Special};

const TIMEOUT: Duration = Duration::from_secs(20);

/// Opens a pty in raw mode, returning the controller and the device path.
fn open_pty() -> (File, String) {
    unsafe {
        let fd = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
        assert!(fd >= 0, "posix_openpt failed");
        assert_eq!(libc::grantpt(fd), 0);
        assert_eq!(libc::unlockpt(fd), 0);
        let name = CStr::from_ptr(libc::ptsname(fd))
            .to_string_lossy()
            .into_owned();
        (File::from_raw_fd(fd), name)
    }
}

#[test]
fn data_flows_both_ways_and_break_is_safe() {
    let (controller, device) = open_pty();
    let mut writer = controller.try_clone().unwrap();
    let (tx, rx) = mpsc::channel();
    let mut reader = controller;
    std::thread::spawn(move || {
        let mut buf = [0u8; 256];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });

    let session = Session::open(&Settings::serial(&device, 9600)).expect("open serial");
    session.send(b"to-device\n");
    session.special(Special::Break);

    let deadline = Instant::now() + TIMEOUT;
    let mut seen = Vec::new();
    while !String::from_utf8_lossy(&seen).contains("to-device") {
        assert!(Instant::now() < deadline, "device never got the data");
        if let Ok(data) = rx.recv_timeout(Duration::from_millis(200)) {
            seen.extend(data);
        }
    }

    writer.write_all(b"from-device\n").unwrap();
    let mut output = Vec::new();
    while !String::from_utf8_lossy(&output).contains("from-device") {
        assert!(Instant::now() < deadline, "no data from device: {output:?}");
        match session.recv_timeout(Duration::from_millis(200)) {
            Ok(Event::Output { data, .. }) => output.extend(data),
            Ok(Event::Closed { reason, .. }) => panic!("closed: {reason:?}"),
            Ok(_) | Err(RecvError::Timeout) => {}
            Err(RecvError::Disconnected) => panic!("engine went away"),
        }
    }
}

#[test]
fn missing_device_is_reported() {
    let result = Session::open(&Settings::serial("/dev/linecon-no-such-port", 9600));
    match result {
        Err(OpenError::Backend(reason)) => {
            assert!(reason.contains("linecon-no-such-port"), "{reason}")
        }
        Err(other) => panic!("unexpected: {other:?}"),
        Ok(_) => panic!("opened a device that does not exist"),
    }
}
