//! Reads each file given `REPEAT` times (default 20) and prints the best time per file:
//! a native driver for profilers (`samply record`) and for timing.
//!
//!   cargo run --release --example profile -- a.docx b.docx
use std::time::Instant;

fn main() {
    let repeat: usize = std::env::var("REPEAT").ok().and_then(|n| n.parse().ok()).unwrap_or(20);
    let mut total = 0.0;
    for path in std::env::args().skip(1) {
        let data = std::fs::read(&path).expect("read");
        let mut best = f64::INFINITY;
        let mut bytes = 0;
        for _ in 0..repeat {
            let start = Instant::now();
            let text = fast_docx_text::extract_raw_text(&data);
            best = best.min(start.elapsed().as_secs_f64() * 1e3);
            bytes = text.map(|t| t.len()).unwrap_or(0);
        }
        total += best;
        if std::env::var("QUIET").is_err() {
            println!("{best:8.2} ms  {bytes:>9} bytes  {path}");
        }
    }
    println!("{total:8.1} ms total (best of {repeat} each)");
}
