use std::env;

mod architecture;
mod code_health;

fn main() {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("code-health") => {
            if let Some(arg) = args.next() {
                eprintln!("code-health: unknown option {arg:?}");
                std::process::exit(2);
            }
            if let Err(code) = code_health::run_code_health() {
                std::process::exit(code);
            }
        }
        Some("architecture") => {
            let mut check = false;
            for arg in args {
                if arg == "--check" {
                    check = true;
                } else {
                    eprintln!("architecture: unknown option {arg:?}");
                    std::process::exit(2);
                }
            }
            if let Err(code) = architecture::run_architecture(check) {
                std::process::exit(code);
            }
        }
        _ => {
            eprintln!("usage: cargo run -p xtask -- code-health|architecture [--check]");
            std::process::exit(2);
        }
    }
}
