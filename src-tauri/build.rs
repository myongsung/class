use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
  println!("cargo:rerun-if-changed=binaries/windows-x64-runtime.zip");
  println!("cargo:rerun-if-changed=resources/models/HyperCLOVAX-SEED-Text-Instruct-0.5B-q4_0.gguf");
  println!("cargo:rerun-if-changed=resources/models/hyperclovax_roosy_Q4_K_M.gguf");

  let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
  if target_os == "windows" {
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is available"));
    let embedded_runtime_path = out_dir.join("embedded-windows-runtime.zip");
    let embedded_hyper_model_path = out_dir.join("embedded-hyper-model.gguf");
    let embedded_roosy_model_path = out_dir.join("embedded-roosy-model.gguf");
    let source_runtime_path = PathBuf::from("binaries").join("windows-x64-runtime.zip");
    let source_hyper_model_path = PathBuf::from("resources")
      .join("models")
      .join("HyperCLOVAX-SEED-Text-Instruct-0.5B-q4_0.gguf");
    let source_roosy_model_path = PathBuf::from("resources")
      .join("models")
      .join("hyperclovax_roosy_Q4_K_M.gguf");

    if source_runtime_path.exists() {
      fs::copy(&source_runtime_path, &embedded_runtime_path)
        .expect("copy embedded windows runtime archive");
    } else {
      panic!(
        "Windows resident runtime archive is missing: {}",
        source_runtime_path.display()
      );
    }

    let copy_model = |source: &PathBuf, target: &PathBuf, label: &str| {
      if !source.exists() {
        panic!("Embedded {} model is missing: {}", label, source.display());
      }
      let metadata = fs::metadata(source)
        .unwrap_or_else(|e| panic!("Cannot stat embedded {} model {}: {e}", label, source.display()));
      if metadata.len() == 0 {
        panic!("Embedded {} model is empty: {}", label, source.display());
      }
      fs::copy(source, target)
        .unwrap_or_else(|e| panic!("Cannot copy embedded {} model {}: {e}", label, source.display()));
    };

    copy_model(&source_hyper_model_path, &embedded_hyper_model_path, "HyperCLOVA-X");
    copy_model(&source_roosy_model_path, &embedded_roosy_model_path, "Roosy-X");
  }

  tauri_build::build()
}
