//! Host estimate of the per-scan cost (engine + key pipeline). Run with
//! `cargo test --release --test engine_bench -- --ignored --nocapture`.
//! The HPM5361 runs at 480 MHz; a scan must finish well inside its 125 µs.

use std::time::Instant;
use zent_core::config::{Global, Profile};
use zent_core::engine::tables::OBJ_TO_SLOT;
use zent_core::engine::Engine;
use zent_core::keys::KeyProcessor;
use zent_core::layout::SLOT_COUNT;

#[test]
#[ignore]
fn per_scan_cost() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/engine");
    let mut g = Global::zeroed();
    let b1 = std::fs::read(format!("{dir}/base_bank1.bin")).unwrap();
    let n = b1.len().min(g.0.len());
    g.0[..n].copy_from_slice(&b1[..n]);
    let mut p = Profile::zeroed();
    let b2 = std::fs::read(format!("{dir}/base_profile.bin")).unwrap();
    let n = b2.len().min(p.0.len());
    p.0[..n].copy_from_slice(&b2[..n]);
    let mut e = Box::new(Engine::new());
    let mut k = Box::new(KeyProcessor::new());
    e.configure(&g, &p);
    k.configure(&p);
    let mut s = [30000u16; SLOT_COUNT];
    for _ in 0..13000 {
        e.scan(&s, &g, &p, false);
        k.update(&e, &p, &g);
    }
    let n = 200_000u32;
    let mut x = 1u32;
    let t = Instant::now();
    let (mut te, mut tk) = (0u128, 0u128);
    for i in 0..n {
        for v in s.iter_mut() {
            x = x.wrapping_mul(1103515245).wrapping_add(12345);
            *v = 30000 + ((x >> 16) % 41) as u16 - 20;
        }
        // a few keys moving
        for key in [46usize, 47, 48, 76] {
            s[OBJ_TO_SLOT[key] as usize] = 30000 - ((i % 2000) as i32 - 1000).unsigned_abs() as u16 * 12;
        }
        let t1 = Instant::now();
        e.scan(std::hint::black_box(&s), &g, &p, false);
        let t2 = Instant::now();
        std::hint::black_box(k.update(&e, &p, &g));
        te += (t2 - t1).as_nanos();
        tk += t2.elapsed().as_nanos();
    }
    println!("engine {} ns, keys {} ns per scan", te / n as u128, tk / n as u128);
    let ns = t.elapsed().as_nanos() as f64 / n as f64;
    println!("engine + keys: {ns:.0} ns per scan on the host ({:.1} % of a 125 µs scan)", ns / 1250.0);
}
