use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
  println!("cargo:rerun-if-changed=binaries/windows-x64-runtime.zip");
  println!("cargo:rerun-if-changed=binaries/llama-server.exe");
  println!("cargo:rerun-if-changed=binaries/windows-x64");
  println!("cargo:rerun-if-changed=resources/models/HyperCLOVAX-SEED-Text-Instruct-0.5B-q4_0.gguf");
  println!("cargo:rerun-if-changed=resources/models/hyperclovax_roosy_Q4_K_M.gguf");

  let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
  if target_os == "windows" {
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is available"));
    let embedded_runtime_path = out_dir.join("embedded-windows-runtime.zip");
    let source_runtime_path = PathBuf::from("binaries").join("windows-x64-runtime.zip");

    if source_runtime_path.exists() {
      fs::copy(&source_runtime_path, &embedded_runtime_path)
        .expect("copy embedded windows runtime archive");
    } else {
      panic!(
        "Windows resident runtime archive is missing: {}",
        source_runtime_path.display()
      );
    }
  }

  tauri_build::build()
}
