//! The binary end to end over its control socket, headless (`--no-uhid`).

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn short_tmp(name: &str) -> PathBuf {
    // sun_path is limited to 108 bytes; keep the socket path short.
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join(format!("zst-{}-{name}", std::process::id()))
}

struct SimProc {
    child: Child,
    sock: PathBuf,
}

impl SimProc {
    fn start(tag: &str, extra: &[&str]) -> Self {
        let sock = short_tmp(&format!("{tag}.sock"));
        let child = Command::new(env!("CARGO_BIN_EXE_zent-sim"))
            .args(["--no-uhid", "--socket"])
            .arg(&sock)
            .args(extra)
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let end = Instant::now() + Duration::from_secs(5);
        while UnixStream::connect(&sock).is_err() {
            assert!(Instant::now() < end, "socket never appeared");
            std::thread::sleep(Duration::from_millis(20));
        }
        Self { child, sock }
    }
    fn conn(&self) -> Conn {
        let s = UnixStream::connect(&self.sock).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        Conn {
            r: BufReader::new(s.try_clone().unwrap()),
            w: s,
        }
    }
}

impl Drop for SimProc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Conn {
    r: BufReader<UnixStream>,
    w: UnixStream,
}

impl Conn {
    /// Send one command; returns the body lines, panics on `err`.
    fn cmd(&mut self, c: &str) -> Vec<String> {
        self.try_cmd(c).unwrap_or_else(|e| panic!("{c}: {e}"))
    }
    fn try_cmd(&mut self, c: &str) -> Result<Vec<String>, String> {
        writeln!(self.w, "{c}").unwrap();
        let mut body = Vec::new();
        loop {
            let mut l = String::new();
            assert!(self.r.read_line(&mut l).unwrap() > 0, "connection closed");
            let l = l.trim_end().to_string();
            if l == "ok" {
                return Ok(body);
            }
            if let Some(e) = l.strip_prefix("err ") {
                return Err(e.to_string());
            }
            body.push(l);
        }
    }
    fn field(&mut self, prefix: &str) -> String {
        self.cmd("state")
            .into_iter()
            .find_map(|l| l.strip_prefix(prefix).map(str::to_string))
            .unwrap_or_default()
    }
    fn wait_ready(&mut self) {
        let end = Instant::now() + Duration::from_secs(10);
        while self.field("engine_ready ") != "1" {
            assert!(Instant::now() < end, "engine never became ready");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    fn wait_hid(&mut self, want: &str) -> Vec<String> {
        let end = Instant::now() + Duration::from_secs(3);
        loop {
            let st = self.cmd("state");
            if st.iter().any(|l| l.starts_with("hid ") && l.contains(want)) {
                return st;
            }
            assert!(Instant::now() < end, "never saw '{want}' in {st:?}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

#[test]
fn socket_commands_drive_the_keyboard() {
    let state = short_tmp("state");
    let sim = SimProc::start("sock", &["--state-dir", state.to_str().unwrap()]);
    let mut c = sim.conn();
    assert!(c.try_cmd("bogus").unwrap_err().contains("unknown command"));
    assert!(
        c.try_cmd("press NoSuchKey")
            .unwrap_err()
            .contains("unknown key")
    );
    assert_eq!(c.field("profile "), "0");
    c.wait_ready();

    c.cmd("press A");
    let st = c.wait_hid("keys=04");
    assert!(st.iter().any(|l| l.starts_with("key 46 A ")), "{st:?}");
    c.cmd("press LShift");
    c.wait_hid("mods=02 keys=04");
    c.cmd("release all");
    c.wait_hid("mods=00 keys=-");

    // partial travel below the 1.6 mm actuation point does not press (F: normal mode; the
    // factory profile 0 has W/A/S/D in rapid trigger, which would actuate at 0.5 mm)
    c.cmd("travel F 0.5");
    std::thread::sleep(Duration::from_millis(100));
    assert!(c.field("hid ").contains("keys=-"));
    c.cmd("travel F 2.5");
    c.wait_hid("keys=09");
    c.cmd("release F");
    c.wait_hid("keys=-");

    // tap: press then automatic release
    c.cmd("tap D 40");
    c.wait_hid("keys=07");
    c.wait_hid("keys=-");

    // leds: 85 triplets
    let leds = c.cmd("leds");
    assert_eq!(leds.len(), 1);
    assert_eq!(leds[0].split_whitespace().count(), 86, "{}", leds[0]);

    // power-up with A held: the engine takes the pressed level as rest → no travel, no key
    let r = c.cmd("hold-at-boot A");
    assert_eq!(r, vec!["rebooted with A held".to_string()]);
    c.wait_ready();
    std::thread::sleep(Duration::from_millis(50));
    let st = c.cmd("state");
    assert!(
        st.iter()
            .any(|l| l.starts_with("hid ") && l.contains("keys=-")),
        "{st:?}"
    );
    let a = st.iter().find(|l| l.starts_with("key 46 ")).unwrap();
    assert!(a.ends_with("travel_um=0"), "{a}");
    c.cmd("release A");

    // a second client works concurrently; quit ends the process
    let mut c2 = sim.conn();
    c2.cmd("help");
    c2.cmd("quit");
    let mut child = sim;
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(st) = child.child.try_wait().unwrap() {
            assert!(st.success());
            break;
        }
        assert!(Instant::now() < end, "sim did not exit on quit");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!child.sock.exists(), "socket removed on exit");
    std::fs::remove_dir_all(&state).unwrap();
}

#[test]
fn script_runs_and_exits() {
    let script = short_tmp("script.txt");
    std::fs::write(
        &script,
        "# demo\nsleep 1700\npress A\nsleep 80\nstate\nrelease A\nsleep 80\nstate\n",
    )
    .unwrap();
    let sock = short_tmp("script.sock");
    let out = Command::new(env!("CARGO_BIN_EXE_zent-sim"))
        .args(["--no-uhid", "--socket"])
        .arg(&sock)
        .arg("--script")
        .arg(&script)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{text}");
    assert!(text.contains("> press A"), "{text}");
    let hids: Vec<&str> = text.lines().filter(|l| l.starts_with("hid ")).collect();
    assert_eq!(hids.len(), 2, "{text}");
    assert!(
        hids[0].contains("keys=04") && hids[1].contains("keys=-"),
        "{hids:?}"
    );
    // the HID change itself is logged
    assert!(text.contains("zent-sim: hid mods=00 keys=04"), "{text}");

    // a failing script exits non-zero
    std::fs::write(&script, "press Nope\n").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_zent-sim"))
        .args(["--no-uhid", "--socket"])
        .arg(&sock)
        .arg("--script")
        .arg(&script)
        .output()
        .unwrap();
    assert!(!out.status.success());
    std::fs::remove_file(&script).unwrap();
}
