//! UART / USB-CDC transport backend (Phase 1).
//!
//! Uses the `serialport` crate to enumerate and open the board's CH34x
//! USB-to-UART bridge (115200 8N1), spawns a background reader thread that
//! reassembles protocol lines and forwards them over a [`LineSender`], and
//! writes JSONL frames back to the board.
//!
//! The reader thread does not decode frames itself; it hands raw lines to the
//! app layer, keeping this module free of any Tauri dependency so it builds and
//! unit-tests on the Linux CI host. On Linux the `serialport` crate is built
//! without the `libudev` feature, so `available_ports()` returns nothing at
//! runtime there — enumeration is exercised on the Windows target only, but the
//! code compiles and the pure matching logic is tested everywhere.
//!
//! macOS lists every USB serial device twice: `/dev/tty.*` (dial-in) and
//! `/dev/cu.*` (callout). Board discovery prefers the `cu.*` node and the UI
//! dropdown hides the `tty.*` twin; on Windows (`COMn`) both are no-ops. The
//! CH340 has a built-in driver on macOS 11+; CH342F/CH343 need WCH's
//! CH34xVCPDriver.

use super::{BoardTransport, LineSender, TransportKind};
use crate::diagnostics;
use crate::protocol::{self, LineAccumulator};
use serde::Serialize;
use serde_json::Value;
use serialport::{SerialPort, SerialPortType};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

/// WCH vendor id shared by the CH340 / CH342 / CH343 bridges.
pub const WCH_VID: u16 = 0x1A86;
/// CH340 product id.
pub const CH340_PID: u16 = 0x7523;
/// CH343 / CH342 product id (the CH342F on PCB v1.2 enumerates here).
pub const CH343_PID: u16 = 0x55D3;

const BAUD_RATE: u32 = 115_200;

#[derive(Clone, Debug, Serialize)]
pub struct SerialPortInfo {
    pub name: String,
    pub kind: String,
}

/// USB-CDC / UART connection to the board.
pub struct UsbSerialTransport {
    endpoint: String,
    port: Box<dyn SerialPort>,
    /// Shared stop flag. `disconnect()` (or dropping the transport) sets it so
    /// the background reader thread exits its blocking read loop and releases
    /// its cloned port handle — otherwise the OS serial port stays open and
    /// board frames keep arriving after a "disconnect".
    stop: Arc<AtomicBool>,
}

impl UsbSerialTransport {
    /// Open `port_name` (or auto-discover a CH34x board port when `None`),
    /// configure 115200 8N1, and spawn the background line reader.
    pub fn connect(port_name: Option<String>, lines: LineSender) -> Result<Self, String> {
        let selected = port_name
            .filter(|port| !port.is_empty())
            .or_else(discover_board_port)
            .ok_or("board USB serial port not found")?;
        diagnostics::record("INFO", "usb-connect", &format!("opening endpoint={selected}"));

        let mut port = serialport::new(&selected, BAUD_RATE)
            .data_bits(serialport::DataBits::Eight)
            .parity(serialport::Parity::None)
            .stop_bits(serialport::StopBits::One)
            .flow_control(serialport::FlowControl::None)
            .timeout(Duration::from_millis(100))
            .open()
            .map_err(|err| format!("open serial port failed: {err}"))?;

        // The CH34x bridge exposes DTR/RTS; on many boards these are wired to
        // EN/BOOT. Assert DTR and keep RTS low so simply opening the port does
        // not trip a reset / download path.
        let _ = port.write_data_terminal_ready(true);
        let _ = port.write_request_to_send(false);

        let reader = port
            .try_clone()
            .map_err(|err| format!("clone serial port failed: {err}"))?;
        let stop = Arc::new(AtomicBool::new(false));
        spawn_reader(reader, lines, stop.clone());
        diagnostics::record("INFO", "usb-connect", "reader started");

        Ok(Self {
            endpoint: selected,
            port,
            stop,
        })
    }
}

impl BoardTransport for UsbSerialTransport {
    fn kind(&self) -> TransportKind {
        TransportKind::Usb
    }

    fn endpoint(&self) -> String {
        self.endpoint.clone()
    }

    fn write_json(&mut self, payload: &Value) -> Result<(), String> {
        let line = protocol::encode_json_line(payload)?;
        /* write_all preserves JSONL frame ordering. Do not flush every frame:
         * on Windows CH340/CH34x, SerialPort::flush waits for the driver TX
         * queue to drain and can block 0.3-2 seconds while the caller still
         * owns the global Companion state mutex. Disconnect performs the one
         * explicit flush needed before closing the port. */
        self.port
            .write_all(&line)
            .map_err(|err| format!("serial write failed: {err}"))
    }

    fn disconnect(&mut self) {
        // Signal the background reader thread to exit its read loop; it will
        // drop its cloned port handle on the way out. Flushing here is a
        // courtesy so any pending write lands before the port closes.
        self.stop.store(true, Ordering::SeqCst);
        let _ = self.port.flush();
    }
}

impl Drop for UsbSerialTransport {
    fn drop(&mut self) {
        // Ensure the reader thread stops even if the transport is dropped
        // without an explicit `disconnect()` (e.g. replaced on reconnect).
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// True when a USB port's identity looks like the board's CH34x bridge.
pub fn is_board_port(vid: u16, pid: u16, product: &str) -> bool {
    if vid != WCH_VID {
        return false;
    }
    if pid == CH340_PID || pid == CH343_PID {
        return true;
    }
    let product = product.to_ascii_lowercase();
    product.contains("ch340") || product.contains("ch343") || product.contains("ch34")
}

/// Human-readable classification used by [`list_serial_ports`].
pub fn port_kind(port_type: &SerialPortType) -> &'static str {
    match port_type {
        SerialPortType::UsbPort(info) => {
            let product = info.product.clone().unwrap_or_default();
            if is_board_port(info.vid, info.pid, &product) {
                "board-cdc"
            } else {
                "usb-serial"
            }
        }
        _ => "serial",
    }
}

const MACOS_DIALIN_PREFIX: &str = "/dev/tty.";
const MACOS_CALLOUT_PREFIX: &str = "/dev/cu.";

/// True for a macOS dial-in device node (`/dev/tty.<name>`).
pub fn is_macos_dialin(name: &str) -> bool {
    name.starts_with(MACOS_DIALIN_PREFIX)
}

/// The `/dev/cu.<name>` twin of a `/dev/tty.<name>` node.
fn callout_twin(dialin: &str) -> Option<String> {
    dialin
        .strip_prefix(MACOS_DIALIN_PREFIX)
        .map(|rest| format!("{MACOS_CALLOUT_PREFIX}{rest}"))
}

/// Pick the port to open from every matching candidate: the first candidate,
/// except that a macOS `tty.*` node whose `cu.*` twin is also present is
/// skipped in favour of the twin. Windows names (`COM5`) are returned in
/// enumeration order, so the Windows result is unchanged.
pub fn prefer_callout(names: Vec<String>) -> Option<String> {
    let skip = |name: &String| {
        is_macos_dialin(name)
            && callout_twin(name).map_or(false, |twin| names.iter().any(|n| *n == twin))
    };
    names
        .iter()
        .find(|name| !skip(name))
        .or_else(|| names.first())
        .cloned()
}

/// Drop each `/dev/tty.*` entry whose `/dev/cu.*` twin is also listed. Keeps
/// order; anything else (including `COM` names) passes through untouched.
pub fn hide_dialin_twins(ports: Vec<SerialPortInfo>) -> Vec<SerialPortInfo> {
    let names: Vec<String> = ports.iter().map(|p| p.name.clone()).collect();
    ports
        .into_iter()
        .filter(|port| {
            !(is_macos_dialin(&port.name)
                && callout_twin(&port.name).map_or(false, |twin| names.contains(&twin)))
        })
        .collect()
}

/// Find the port that looks like the board's CH34x bridge (on macOS the
/// callout `cu.*` node is preferred over its `tty.*` twin).
pub fn discover_board_port() -> Option<String> {
    let matches: Vec<String> = serialport::available_ports()
        .ok()?
        .into_iter()
        .filter_map(|port| {
            if let SerialPortType::UsbPort(info) = &port.port_type {
                let product = info.product.clone().unwrap_or_default();
                if is_board_port(info.vid, info.pid, &product) {
                    return Some(port.port_name);
                }
            }
            None
        })
        .collect();
    prefer_callout(matches)
}

/// Enumerate all serial ports with a coarse classification for the UI dropdown.
/// macOS `tty.*` twins of a listed `cu.*` node are hidden.
pub fn list_serial_ports() -> Result<Vec<SerialPortInfo>, String> {
    let ports =
        serialport::available_ports().map_err(|err| format!("list serial ports failed: {err}"))?;
    Ok(hide_dialin_twins(
        ports
            .into_iter()
            .map(|port| SerialPortInfo {
                kind: port_kind(&port.port_type).to_string(),
                name: port.port_name,
            })
            .collect(),
    ))
}

/// Read bytes off the port, reassemble complete lines, and forward them.
/// Exits (dropping the cloned port) as soon as `stop` is set, or when the
/// receiver on the other end of `lines` is dropped.
fn spawn_reader(mut reader: Box<dyn SerialPort>, lines: LineSender, stop: Arc<AtomicBool>) {
    thread::spawn(move || {
        diagnostics::record("INFO", "usb-reader", "thread started");
        let mut acc = LineAccumulator::new();
        let mut chunk = [0u8; 512];
        let mut last_error_log = Instant::now() - Duration::from_secs(5);
        loop {
            if stop.load(Ordering::SeqCst) {
                diagnostics::record("INFO", "usb-reader", "stop requested");
                return; // disconnect requested -> release the cloned port
            }
            match reader.read(&mut chunk) {
                Ok(0) => thread::sleep(Duration::from_millis(20)),
                Ok(n) => {
                    for line in acc.push(&chunk[..n]) {
                        if lines.send(line).is_err() {
                            diagnostics::record("INFO", "usb-reader", "receiver dropped");
                            return; // receiver dropped -> connection gone
                        }
                    }
                }
                Err(ref err) if err.kind() == std::io::ErrorKind::TimedOut => {
                    // Idle read timeout is normal; loop back to re-check `stop`.
                }
                Err(error) => {
                    if last_error_log.elapsed() >= Duration::from_secs(5) {
                        diagnostics::record("WARN", "usb-reader", &format!("read failed: {error}"));
                        last_error_log = Instant::now();
                    }
                    thread::sleep(Duration::from_millis(50));
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_ch340_and_ch343_by_pid() {
        assert!(is_board_port(WCH_VID, CH340_PID, ""));
        assert!(is_board_port(WCH_VID, CH343_PID, ""));
    }

    #[test]
    fn matches_wch_by_product_name() {
        assert!(is_board_port(WCH_VID, 0x0001, "USB Serial (CH343)"));
        assert!(is_board_port(WCH_VID, 0x0001, "ch340 uart"));
    }

    #[test]
    fn rejects_other_vendors_and_names() {
        assert!(!is_board_port(0x0403, CH340_PID, "FTDI")); // right pid, wrong vid
        assert!(!is_board_port(WCH_VID, 0x9999, "Some Other Device"));
    }

    fn info(name: &str) -> SerialPortInfo {
        SerialPortInfo {
            name: name.to_string(),
            kind: "serial".to_string(),
        }
    }

    fn names(ports: &[SerialPortInfo]) -> Vec<&str> {
        ports.iter().map(|p| p.name.as_str()).collect()
    }

    #[test]
    fn is_macos_dialin_matches_only_tty_dot() {
        assert!(is_macos_dialin("/dev/tty.usbserial-1410"));
        assert!(!is_macos_dialin("/dev/cu.usbserial-1410"));
        assert!(!is_macos_dialin("/dev/ttyUSB0"));
        assert!(!is_macos_dialin("COM5"));
    }

    #[test]
    fn prefer_callout_picks_cu_from_a_tty_cu_pair() {
        for order in [
            vec!["/dev/tty.usbserial-1410", "/dev/cu.usbserial-1410"],
            vec!["/dev/cu.usbserial-1410", "/dev/tty.usbserial-1410"],
        ] {
            let list = order.into_iter().map(String::from).collect();
            assert_eq!(
                prefer_callout(list).as_deref(),
                Some("/dev/cu.usbserial-1410")
            );
        }
    }

    #[test]
    fn prefer_callout_keeps_lone_tty_and_windows_order() {
        assert_eq!(
            prefer_callout(vec!["/dev/tty.usbserial-1".to_string()]).as_deref(),
            Some("/dev/tty.usbserial-1")
        );
        assert_eq!(
            prefer_callout(vec!["COM5".to_string(), "COM3".to_string()]).as_deref(),
            Some("COM5")
        );
        assert_eq!(prefer_callout(Vec::new()), None);
    }

    #[test]
    fn hide_dialin_twins_drops_tty_only_when_twin_exists() {
        let ports = vec![
            info("/dev/tty.usbserial-1"),
            info("/dev/cu.usbserial-1"),
            info("/dev/tty.lonely"),
            info("COM3"),
            info("/dev/cu.Bluetooth-Incoming-Port"),
            info("/dev/tty.Bluetooth-Incoming-Port"),
        ];
        let out = hide_dialin_twins(ports);
        assert_eq!(
            names(&out),
            vec![
                "/dev/cu.usbserial-1",
                "/dev/tty.lonely",
                "COM3",
                "/dev/cu.Bluetooth-Incoming-Port"
            ]
        );
    }

    #[test]
    fn hide_dialin_twins_leaves_windows_names_untouched() {
        let out = hide_dialin_twins(vec![info("COM5"), info("COM3")]);
        assert_eq!(names(&out), vec!["COM5", "COM3"]);
    }

    #[test]
    fn port_kind_classifies_usb_variants() {
        use serialport::UsbPortInfo;
        let board = SerialPortType::UsbPort(UsbPortInfo {
            vid: WCH_VID,
            pid: CH343_PID,
            serial_number: None,
            manufacturer: None,
            product: Some("CH343".into()),
        });
        assert_eq!(port_kind(&board), "board-cdc");

        let other = SerialPortType::UsbPort(UsbPortInfo {
            vid: 0x0403,
            pid: 0x6001,
            serial_number: None,
            manufacturer: None,
            product: Some("FT232".into()),
        });
        assert_eq!(port_kind(&other), "usb-serial");

        assert_eq!(port_kind(&SerialPortType::Unknown), "serial");
    }
}
