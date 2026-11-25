//! `zent-sim`: run the firmware core on the host with a virtual HID device.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use zent_sim::command::Command;
use zent_sim::control;
use zent_sim::sim::{self, Options, Request, Sim, log};
use zent_sim::store::DirStorage;

const USAGE: &str = "\
usage: zent-sim [options]

  --state-dir DIR      persist config banks here (default: a temp dir removed on exit)
  --load-backup DIR    seed banks from a tools/dump_all.py backup (backups/<ts>) before boot
  --socket PATH        control socket (default: $XDG_RUNTIME_DIR/zent75-sim.sock)
  --script FILE        run control commands from FILE, then quit (see --stay)
  --stay               keep running after --script finished
  --noise N            sensor noise, +-N ADC counts (default 30)
  --slew MM_PER_S      key travel speed, 0 = instant (default 200)
  --seed N             per-key sensor variation seed (default 1)
  --no-uhid            no virtual HID device (control socket only)
  --hid-keyboard       also create a uhid keyboard that TYPES INTO YOUR SESSION (off by default)
  --sudo-chmod         run `sudo -n chmod 0666` on the hidraw node if it is not accessible
  --bootloader-ms N    how long the device stays away after BE FC (default 2000)
  --verbose            log every vendor request
";

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}

struct Args {
    opts: Options,
    state_dir: Option<PathBuf>,
    load_backup: Option<PathBuf>,
    socket: PathBuf,
    script: Option<PathBuf>,
    stay: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        opts: Options {
            uhid: true,
            ..Options::default()
        },
        state_dir: None,
        load_backup: None,
        socket: std::env::var_os("XDG_RUNTIME_DIR")
            .map(|d| PathBuf::from(d).join("zent75-sim.sock"))
            .unwrap_or_else(|| {
                // SAFETY: getuid() has no preconditions and cannot fail.
                let uid = unsafe { libc::getuid() };
                std::env::temp_dir().join(format!("zent75-sim-{uid}.sock"))
            }),
        script: None,
        stay: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut val = || it.next().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--state-dir" => a.state_dir = Some(val()?.into()),
            "--load-backup" => a.load_backup = Some(val()?.into()),
            "--socket" => a.socket = val()?.into(),
            "--script" => a.script = Some(val()?.into()),
            "--stay" => a.stay = true,
            "--noise" => a.opts.noise = val()?.parse().map_err(|_| "bad --noise")?,
            "--slew" => a.opts.slew_mm_s = val()?.parse().map_err(|_| "bad --slew")?,
            "--seed" => a.opts.seed = val()?.parse().map_err(|_| "bad --seed")?,
            "--no-uhid" => a.opts.uhid = false,
            "--hid-keyboard" => a.opts.hid_keyboard = true,
            "--sudo-chmod" => a.opts.sudo_chmod = true,
            "--bootloader-ms" => {
                a.opts.bootloader_delay =
                    Duration::from_millis(val()?.parse().map_err(|_| "bad --bootloader-ms")?)
            }
            "--verbose" => a.opts.verbose = true,
            "-h" | "--help" => return Err(String::new()),
            _ => return Err(format!("unknown option {flag}")),
        }
    }
    Ok(a)
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("zent-sim: {e}");
            }
            eprint!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    // SAFETY: the handler only stores to an atomic.
    unsafe {
        libc::signal(libc::SIGINT, on_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t);
    }

    let (state_dir, temp_state) = match &args.state_dir {
        Some(d) => (d.clone(), false),
        None => (
            std::env::temp_dir().join(format!("zent75-sim-state-{}", std::process::id())),
            true,
        ),
    };
    let mut storage = match DirStorage::open(&state_dir) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("zent-sim: state dir {}: {e}", state_dir.display());
            return ExitCode::FAILURE;
        }
    };
    log(format!(
        "state-dir {}{}",
        state_dir.display(),
        if temp_state { " (temporary)" } else { "" }
    ));
    if let Some(b) = &args.load_backup {
        match zent_sim::backup::load_backup(b, &mut storage) {
            Ok(s) => {
                log(format!(
                    "loaded backup {} (fw {}, {} regions, active profile {})",
                    b.display(),
                    s.firmware,
                    s.regions,
                    s.active_profile
                ));
                for w in s.skipped {
                    log(format!("  skipped: {w}"));
                }
            }
            Err(e) => {
                eprintln!("zent-sim: --load-backup: {e}");
                return ExitCode::FAILURE;
            }
        }
    }

    let listener = match control::bind(&args.socket) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("zent-sim: socket {}: {e}", args.socket.display());
            return ExitCode::FAILURE;
        }
    };
    log(format!("socket {}", args.socket.display()));

    let mut sim = match Sim::new(args.opts.clone(), storage) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "zent-sim: cannot create the uhid device: {e} (need rw on /dev/uhid, or use --no-uhid)"
            );
            let _ = std::fs::remove_file(&args.socket);
            return ExitCode::FAILURE;
        }
    };

    let (tx, rx) = mpsc::channel::<Request>();
    control::spawn_server(listener, tx.clone());
    let script_result = std::sync::Arc::new(std::sync::Mutex::new(Ok(())));
    if let Some(path) = &args.script {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("zent-sim: script {}: {e}", path.display());
                return ExitCode::FAILURE;
            }
        };
        let (tx, stay, res) = (tx.clone(), args.stay, script_result.clone());
        std::thread::spawn(move || {
            let mut out = std::io::stdout();
            let r = control::run_script(&text, &tx, &mut out);
            let quit = matches!(r, Ok(true));
            if let Err(e) = &r {
                eprintln!("zent-sim: script failed: {e}");
            }
            *res.lock().unwrap() = r.map(|_| ());
            if !quit && !stay {
                let (rtx, _rrx) = mpsc::channel();
                let _ = tx.send(Request {
                    cmd: Command::Quit,
                    reply: rtx,
                });
            }
        });
    }
    drop(tx);

    sim::run(&mut sim, &rx, &STOP);

    drop(sim);
    let _ = std::fs::remove_file(&args.socket);
    if temp_state {
        let _ = std::fs::remove_dir_all(&state_dir);
    }
    if script_result.lock().unwrap().is_err() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
