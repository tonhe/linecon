//! Backends against a loopback peer. Runs on every target.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};

use linecon_engine::settings::Settings;
use linecon_engine::{Event, OpenError, RecvError, Session};

const TIMEOUT: Duration = Duration::from_secs(20);

/// Echoes everything back until the peer closes.
fn echo_peer() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 1024];
        while let Ok(n) = stream.read(&mut buf) {
            if n == 0 || stream.write_all(&buf[..n]).is_err() {
                break;
            }
        }
    });
    port
}

fn read_until(session: &Session, needle: &[u8]) -> Vec<u8> {
    let deadline = Instant::now() + TIMEOUT;
    let mut seen = Vec::new();
    while Instant::now() < deadline {
        match session.recv_timeout(Duration::from_millis(200)) {
            Ok(Event::Output { data, .. }) => {
                seen.extend(data);
                if seen.windows(needle.len()).any(|w| w == needle) {
                    return seen;
                }
            }
            Ok(Event::Closed { reason, .. }) => panic!("closed early: {reason:?}"),
            Ok(_) | Err(RecvError::Timeout) => {}
            Err(RecvError::Disconnected) => panic!("engine went away"),
        }
    }
    panic!("timed out; saw {:?}", String::from_utf8_lossy(&seen));
}

#[test]
fn raw_round_trip() {
    let port = echo_peer();
    let session = Session::open(&Settings::raw("127.0.0.1", port)).unwrap();
    session.send(b"hello-raw\n");
    read_until(&session, b"hello-raw");
}

#[test]
fn telnet_round_trip() {
    let port = echo_peer();
    let session = Session::open(&Settings::telnet("127.0.0.1", port)).unwrap();
    session.send(b"hello-telnet\r\n");
    read_until(&session, b"hello-telnet");
}

#[test]
fn peer_close_reports_closed() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.write_all(b"bye\n").unwrap();
    });
    let session = Session::open(&Settings::raw("127.0.0.1", port)).unwrap();
    let deadline = Instant::now() + TIMEOUT;
    while Instant::now() < deadline {
        match session.recv_timeout(Duration::from_millis(200)) {
            Ok(Event::Closed { .. }) => return,
            Ok(_) | Err(RecvError::Timeout) => {}
            Err(RecvError::Disconnected) => panic!("engine went away"),
        }
    }
    panic!("no Closed event");
}

#[test]
fn refused_connection_is_reported() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    match Session::open(&Settings::raw("127.0.0.1", port)) {
        Err(OpenError::Backend(_)) => {}
        Err(other) => panic!("unexpected: {other:?}"),
        Ok(session) => {
            let deadline = Instant::now() + TIMEOUT;
            while Instant::now() < deadline {
                match session.recv_timeout(Duration::from_millis(200)) {
                    Ok(Event::Closed { .. }) => return,
                    Ok(_) | Err(RecvError::Timeout) => {}
                    Err(RecvError::Disconnected) => panic!("engine went away"),
                }
            }
            panic!("no Closed event");
        }
    }
}

#[test]
fn many_sessions_stay_separate() {
    let sessions: Vec<_> = (0..8)
        .map(|i| {
            let session = Session::open(&Settings::raw("127.0.0.1", echo_peer())).unwrap();
            session.send(format!("session-{i}\n").as_bytes());
            (i, session)
        })
        .collect();
    for (i, session) in &sessions {
        let seen = read_until(session, format!("session-{i}").as_bytes());
        for other in 0..8 {
            if other != *i {
                assert!(!String::from_utf8_lossy(&seen).contains(&format!("session-{other}")));
            }
        }
    }
}
