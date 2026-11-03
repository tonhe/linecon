//! Finds serial ports and offers settings for common console cables. Scans device
//! names directly: a libudev dependency would break the portable Linux binary.

use std::sync::Mutex;
use std::time::{Duration, Instant};

#[cfg_attr(windows, allow(dead_code))]
/// Picks console-capable devices out of a directory listing. On macOS the `cu.`
/// names are the ones that open without waiting for carrier.
pub fn filter_device_names<'a>(names: impl Iterator<Item = &'a str>) -> Vec<String> {
    const PREFIXES: [&str; 5] = ["ttyUSB", "ttyACM", "cu.usb", "cu.serial", "cu.SLAB"];
    let mut found: Vec<String> = names
        .filter(|n| PREFIXES.iter().any(|p| n.starts_with(p)) || n.starts_with("cu.wch"))
        .map(|n| format!("/dev/{n}"))
        .collect();
    found.sort();
    found
}

#[cfg_attr(not(windows), allow(dead_code))]
/// Parses `reg query HKLM\HARDWARE\DEVICEMAP\SERIALCOMM` output into port names.
pub fn parse_serialcomm(text: &str) -> Vec<String> {
    let mut found: Vec<String> = text
        .lines()
        .filter(|l| l.contains("REG_SZ"))
        .filter_map(|l| l.split_whitespace().last())
        .filter(|p| p.starts_with("COM"))
        .map(str::to_string)
        .collect();
    found.sort_by_key(|p| p[3..].parse::<u32>().unwrap_or(u32::MAX));
    found
}

/// The serial ports present, refreshed every few seconds. The UI asks on every
/// frame while a serial session's settings are open, and scanning is not free.
pub fn list_ports() -> Vec<String> {
    static CACHE: Mutex<Option<(Instant, Vec<String>)>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap();
    match cache.as_ref() {
        Some((at, ports)) if at.elapsed() < Duration::from_secs(3) => ports.clone(),
        _ => {
            let ports = scan();
            *cache = Some((Instant::now(), ports.clone()));
            ports
        }
    }
}

#[cfg(windows)]
fn scan() -> Vec<String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new("reg")
        .args(["query", r"HKLM\HARDWARE\DEVICEMAP\SERIALCOMM"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map(|out| parse_serialcomm(&String::from_utf8_lossy(&out.stdout)))
        .unwrap_or_default()
}

#[cfg(not(windows))]
fn scan() -> Vec<String> {
    let Ok(entries) = std::fs::read_dir("/dev") else {
        return Vec::new();
    };
    let names: Vec<String> = entries
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    filter_device_names(names.iter().map(String::as_str))
}

/// Line settings for common console cables.
pub struct ConsolePreset {
    pub label: &'static str,
    pub baud: u32,
    pub data_bits: u8,
    pub parity: linecon_config::Parity,
}

pub const CONSOLE_PRESETS: [ConsolePreset; 4] = [
    ConsolePreset {
        label: "9600 8N1: Cisco, Juniper, Arista, HP",
        baud: 9600,
        data_bits: 8,
        parity: linecon_config::Parity::None,
    },
    ConsolePreset {
        label: "115200 8N1: MikroTik, Ubiquiti, Linux",
        baud: 115_200,
        data_bits: 8,
        parity: linecon_config::Parity::None,
    },
    ConsolePreset {
        label: "19200 8N1: older gear",
        baud: 19_200,
        data_bits: 8,
        parity: linecon_config::Parity::None,
    },
    ConsolePreset {
        label: "9600 7E1: old terminal servers",
        baud: 9600,
        data_bits: 7,
        parity: linecon_config::Parity::Even,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_console_devices_only() {
        let names = [
            "ttyUSB1",
            "ttyUSB0",
            "ttyACM0",
            "tty1",
            "sda",
            "cu.usbserial-A1",
            "tty.usbserial-A1",
            "cu.Bluetooth",
        ];
        assert_eq!(
            filter_device_names(names.into_iter()),
            [
                "/dev/cu.usbserial-A1",
                "/dev/ttyACM0",
                "/dev/ttyUSB0",
                "/dev/ttyUSB1"
            ]
        );
    }

    #[test]
    fn parses_the_windows_port_list() {
        let text = "\r\nHKEY_LOCAL_MACHINE\\HARDWARE\\DEVICEMAP\\SERIALCOMM\r\n\
    \\Device\\VCP0    REG_SZ    COM10\r\n    \\Device\\Serial0    REG_SZ    COM3\r\n";
        assert_eq!(parse_serialcomm(text), ["COM3", "COM10"]);
        assert!(parse_serialcomm("ERROR: not found").is_empty());
    }
}
