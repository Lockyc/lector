#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("validate") => std::process::exit(lector_lib::validate_cli(
            args.get(2).map(std::path::PathBuf::from),
        )),
        Some("fmt") => {
            let mut check = false;
            let mut path = None;
            for a in &args[2..] {
                match a.as_str() {
                    "--check" => check = true,
                    p => path = Some(std::path::PathBuf::from(p)),
                }
            }
            // `fmt` is schema-free — delegate to config-core's shared fmt_cli (re-exported by
            // lector-config); only the default config path is lector's.
            let path = path.unwrap_or_else(lector_config::resolve_config_path);
            std::process::exit(lector_config::fmt_cli(check, &path));
        }
        Some(other) => {
            eprintln!(
                "unknown command: {other}\nusage: lector [validate [path] | fmt [--check] [path]]"
            );
            std::process::exit(2);
        }
        None => lector_lib::run(),
    }
}
