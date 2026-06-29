//! Client/FFI code generator for LEZ programs.
//! Wraps lez-client-gen to support --idl-dir for batch generation.

use std::path::PathBuf;

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let mut idl_dir: Option<PathBuf> = None;
    let mut out_dir: Option<PathBuf> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--idl-dir" => {
                idl_dir = Some(PathBuf::from(args.get(i + 1).ok_or("--idl-dir requires value")?));
                i += 2;
            }
            "--out-dir" => {
                out_dir = Some(PathBuf::from(args.get(i + 1).ok_or("--out-dir requires value")?));
                i += 2;
            }
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }

    let idl_dir = idl_dir.ok_or("missing --idl-dir")?;
    let out_dir = out_dir.ok_or("missing --out-dir")?;

    std::fs::create_dir_all(&out_dir)?;

    for entry in std::fs::read_dir(&idl_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().map(|e| e == "json").unwrap_or(false) {
            let json = std::fs::read_to_string(&path)?;
            let output = spel_client_gen::generate_from_idl_json(&json)?;

            let program_name: String = {
                let idl: serde_json::Value = serde_json::from_str(&json)?;
                idl["name"].as_str().unwrap_or("program").replace('-', "_")
            };

            let client_path = out_dir.join(format!("{}_client.rs", program_name));
            let ffi_path = out_dir.join(format!("{}_ffi.rs", program_name));
            let header_path = out_dir.join(format!("{}.h", program_name));

            std::fs::write(&client_path, &output.client_code)?;
            std::fs::write(&ffi_path, &output.ffi_code)?;
            std::fs::write(&header_path, &output.header)?;

            println!("Generated from {}:", path.display());
            println!("  Client: {}", client_path.display());
            println!("  FFI:    {}", ffi_path.display());
            println!("  Header: {}", header_path.display());
        }
    }

    Ok(())
}
