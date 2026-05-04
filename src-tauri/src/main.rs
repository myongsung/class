#![recursion_limit = "512"]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod engine;
mod commands;
mod drace;

#[cfg(target_os = "windows")]
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
#[cfg(target_os = "windows")]
use base64::Engine;
#[cfg(target_os = "macos")]
use std::env;
use std::fs;
#[cfg(target_os = "windows")]
use std::fs::File;
#[cfg(target_os = "windows")]
use std::io::{self, Cursor};
#[cfg(any(target_os = "windows", target_os = "macos"))]
use std::path::{Path, PathBuf};
#[cfg(any(target_os = "windows", target_os = "macos"))]
use std::process::Command;
#[cfg(target_os = "windows")]
use tauri::Manager;
use tauri::{command, AppHandle};

#[cfg(target_os = "windows")]
const UPDATE_REPO_OWNER: &str = "myongsung";
#[cfg(target_os = "windows")]
const UPDATE_REPO_NAME: &str = "class";
#[cfg(target_os = "windows")]
const UPDATE_ASSET_NAME: &str = "roosycozy-x86_64-pc-windows-msvc.zip";
#[cfg(target_os = "windows")]
const WINDOWS_BUNDLE_SUPPORT_DIR_NAME: &str = "RoosyCozy";
#[cfg(target_os = "windows")]
const WINDOWS_RUNTIME_URL: &str =
    "https://github.com/myongsung/class/releases/latest/download/roosycozy-windows-runtime.zip";
#[cfg(target_os = "windows")]
const WINDOWS_RUNTIME_MARKER_FILENAME: &str = ".runtime-ready";
#[cfg(target_os = "windows")]
const CURRENT_WINDOWS_APP_ID: &str = "co.roosycozy.desktop";
#[cfg(target_os = "windows")]
const LEGACY_WINDOWS_APP_ID: &str = "co.roosycozy.app";
#[cfg(target_os = "windows")]
const EMBEDDED_WINDOWS_RUNTIME_ZIP: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/embedded-windows-runtime.zip"));
#[cfg(target_os = "macos")]
const LEGACY_MAC_APP_ID: &str = "co.roosycozy.app";
#[cfg(target_os = "macos")]
const CURRENT_MAC_APP_ID: &str = "co.roosycozy.desktop";

#[cfg(target_os = "windows")]
#[derive(Debug, serde::Deserialize)]
struct GithubReleaseAsset {
    name: String,
    browser_download_url: String,
}

#[cfg(target_os = "windows")]
#[derive(Debug, serde::Deserialize)]
struct GithubRelease {
    tag_name: String,
    assets: Vec<GithubReleaseAsset>,
}

#[cfg(target_os = "windows")]
fn normalize_release_version(value: &str) -> String {
    value.trim().trim_start_matches('v').to_string()
}

#[cfg(target_os = "windows")]
fn parse_version_triplet(value: &str) -> [u32; 3] {
    let mut out = [0u32; 3];
    for (idx, part) in normalize_release_version(value).split('.').take(3).enumerate() {
        out[idx] = part.parse::<u32>().unwrap_or(0);
    }
    out
}

#[cfg(target_os = "windows")]
fn latest_github_release() -> Result<GithubRelease, String> {
    let url = format!(
        "https://api.github.com/repos/{}/{}/releases/latest",
        UPDATE_REPO_OWNER, UPDATE_REPO_NAME
    );
    reqwest::blocking::Client::builder()
        .user_agent("roosycozy-updater/1.0")
        .build()
        .map_err(|e| format!("업데이트 클라이언트를 준비하지 못했어요: {}", e))?
        .get(url)
        .send()
        .map_err(|e| format!("최신 릴리즈를 확인하지 못했어요: {}", e))?
        .error_for_status()
        .map_err(|e| format!("최신 릴리즈 응답이 올바르지 않아요: {}", e))?
        .json::<GithubRelease>()
        .map_err(|e| format!("최신 릴리즈 정보를 읽지 못했어요: {}", e))
}

#[cfg(target_os = "windows")]
fn download_release_zip(url: &str, target_zip: &Path) -> Result<(), String> {
    let mut response = reqwest::blocking::Client::builder()
        .user_agent("roosycozy-updater/1.0")
        .build()
        .map_err(|e| format!("업데이트 다운로드 클라이언트를 준비하지 못했어요: {}", e))?
        .get(url)
        .send()
        .map_err(|e| format!("업데이트 파일을 받지 못했어요: {}", e))?
        .error_for_status()
        .map_err(|e| format!("업데이트 파일 응답이 올바르지 않아요: {}", e))?;

    if let Some(parent) = target_zip.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("임시 업데이트 폴더를 만들지 못했어요: {}", e))?;
    }

    let mut file = File::create(target_zip).map_err(|e| format!("업데이트 zip을 만들지 못했어요: {}", e))?;
    io::copy(&mut response, &mut file).map_err(|e| format!("업데이트 zip 저장에 실패했어요: {}", e))?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn windows_staging_dir() -> PathBuf {
    windows_updates_root_dir().join("staging")
}

#[cfg(target_os = "windows")]
fn windows_ps_escape(value: &Path) -> String {
    value.to_string_lossy().replace('\'', "''")
}

#[cfg(target_os = "windows")]
fn windows_ps_encoded(script: &str) -> String {
    let utf16: Vec<u16> = script.encode_utf16().collect();
    let mut bytes = Vec::with_capacity(utf16.len() * 2);
    for unit in utf16 {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    BASE64_STANDARD.encode(bytes)
}

#[cfg(target_os = "windows")]
fn extract_release_zip(zip_path: &Path, target_dir: &Path) -> Result<(), String> {
    fs::create_dir_all(target_dir).map_err(|e| format!("업데이트 압축 해제 폴더를 만들지 못했어요: {}", e))?;
    let file = File::open(zip_path).map_err(|e| format!("업데이트 zip을 열지 못했어요: {}", e))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("업데이트 zip 형식이 올바르지 않아요: {}", e))?;

    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|e| format!("업데이트 zip 항목을 읽지 못했어요: {}", e))?;
        let out_path = target_dir.join(entry.mangled_name());

        if entry.is_dir() {
            fs::create_dir_all(&out_path).map_err(|e| format!("업데이트 폴더를 만들지 못했어요: {}", e))?;
            continue;
        }

        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("업데이트 파일 폴더를 만들지 못했어요: {}", e))?;
        }

        let mut out_file = File::create(&out_path).map_err(|e| format!("업데이트 파일을 만들지 못했어요: {}", e))?;
        io::copy(&mut entry, &mut out_file).map_err(|e| format!("업데이트 파일 저장에 실패했어요: {}", e))?;
    }

    Ok(())
}

#[cfg(target_os = "windows")]
fn extract_zip_archive_to_dir<R: io::Read + io::Seek>(
    reader: R,
    target_dir: &Path,
) -> Result<(), String> {
    fs::create_dir_all(target_dir).map_err(|e| format!("압축 해제 폴더를 만들지 못했어요: {}", e))?;
    let mut archive =
        zip::ZipArchive::new(reader).map_err(|e| format!("내장 runtime zip 형식이 올바르지 않아요: {}", e))?;

    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|e| format!("내장 runtime zip 항목을 읽지 못했어요: {}", e))?;
        let out_path = target_dir.join(entry.mangled_name());

        if entry.is_dir() {
            fs::create_dir_all(&out_path).map_err(|e| format!("runtime 폴더를 만들지 못했어요: {}", e))?;
            continue;
        }

        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("runtime 파일 폴더를 만들지 못했어요: {}", e))?;
        }

        let mut out_file =
            File::create(&out_path).map_err(|e| format!("runtime 파일을 만들지 못했어요: {}", e))?;
        io::copy(&mut entry, &mut out_file).map_err(|e| format!("runtime 파일 저장에 실패했어요: {}", e))?;
    }

    Ok(())
}

#[cfg(target_os = "windows")]
fn copy_dir_recursive(source: &Path, target: &Path) -> Result<(), String> {
    if !source.exists() {
        return Ok(());
    }
    fs::create_dir_all(target).map_err(|e| format!("업데이트 대상 폴더를 만들지 못했어요: {}", e))?;
    for entry in fs::read_dir(source).map_err(|e| format!("업데이트 폴더를 읽지 못했어요: {}", e))? {
        let entry = entry.map_err(|e| format!("업데이트 폴더 항목을 읽지 못했어요: {}", e))?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        if source_path.is_dir() {
            copy_dir_recursive(&source_path, &target_path)?;
        } else {
            if let Some(parent) = target_path.parent() {
                fs::create_dir_all(parent).map_err(|e| format!("업데이트 파일 폴더를 만들지 못했어요: {}", e))?;
            }
            fs::copy(&source_path, &target_path).map_err(|e| format!("업데이트 파일 복사에 실패했어요: {}", e))?;
        }
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn schedule_windows_exe_swap(current_exe: &Path, staged_exe: &Path) -> Result<(), String> {
    let staging_dir = windows_staging_dir();
    fs::create_dir_all(&staging_dir)
        .map_err(|e| format!("업데이트 staging 폴더를 만들지 못했어요: {}", e))?;

    let old_exe = current_exe.with_extension("exe.old");
    let pid = std::process::id();
    let script = format!(
        "$target = '{target}'\n\
$staged = '{staged}'\n\
$old = '{old}'\n\
while (Get-Process -Id {pid} -ErrorAction SilentlyContinue) {{ Start-Sleep -Milliseconds 500 }}\n\
if (Test-Path $old) {{ Remove-Item -LiteralPath $old -Force -ErrorAction SilentlyContinue }}\n\
if (Test-Path $target) {{ Move-Item -LiteralPath $target -Destination $old -Force }}\n\
Move-Item -LiteralPath $staged -Destination $target -Force\n\
Start-Process -FilePath $target\n",
        target = windows_ps_escape(current_exe),
        staged = windows_ps_escape(staged_exe),
        old = windows_ps_escape(&old_exe),
        pid = pid
    );
    let encoded = windows_ps_encoded(&script);
    Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-WindowStyle",
            "Hidden",
            "-EncodedCommand",
            &encoded,
        ])
        .spawn()
        .map_err(|e| format!("업데이트 적용 스크립트를 실행하지 못했어요: {}", e))?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn windows_sidecar_storage_dir(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|e| format!("AppData 폴더를 찾지 못했어요: {}", e))?
        .join("sidecar"))
}

#[cfg(target_os = "windows")]
fn windows_resources_storage_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let _ = app;
    Ok(windows_shared_root().join("models"))
}

#[cfg(target_os = "windows")]
fn windows_updates_root_dir() -> PathBuf {
    windows_shared_root().join("updates")
}

#[cfg(target_os = "windows")]
fn windows_temp_work_dir(prefix: &str) -> PathBuf {
    windows_updates_root_dir().join(format!("{}-{}", prefix, std::process::id()))
}

#[cfg(target_os = "windows")]
fn windows_shared_root() -> PathBuf {
    let public_root = std::env::var_os("PUBLIC")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Users\Public"));
    public_root
        .join("Documents")
        .join("RoosyCozy")
        .join("co.roosycozy.desktop")
}

#[cfg(target_os = "windows")]
fn windows_appdata_root(app_id: &str) -> PathBuf {
    windows_env_dir("APPDATA")
        .unwrap_or_else(|| PathBuf::from(r"C:\Users\Default\AppData\Roaming"))
        .join(app_id)
}

#[cfg(target_os = "windows")]
fn windows_legacy_shared_root() -> PathBuf {
    let public_root = std::env::var_os("PUBLIC")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Users\Public"));
    public_root
        .join("Documents")
        .join("RoosyCozy")
        .join(LEGACY_WINDOWS_APP_ID)
}

#[cfg(target_os = "windows")]
fn windows_sidecar_required_files() -> [&'static str; 2] {
    [
        "llama.dll",
        "mtmd.dll",
    ]
}

#[cfg(target_os = "windows")]
fn windows_resident_server_required_files() -> [&'static str; 1] {
    ["llama-server.exe"]
}

#[cfg(target_os = "windows")]
fn windows_msvc_runtime_files() -> [&'static str; 4] {
    [
        "msvcp140.dll",
        "vcruntime140.dll",
        "vcruntime140_1.dll",
        "concrt140.dll",
    ]
}

#[cfg(target_os = "windows")]
fn windows_runtime_marker_path(sidecar_dir: &Path) -> PathBuf {
    sidecar_dir.join(WINDOWS_RUNTIME_MARKER_FILENAME)
}

#[cfg(target_os = "windows")]
fn write_windows_runtime_marker(sidecar_dir: &Path, source: &str) -> Result<(), String> {
    fs::write(
        windows_runtime_marker_path(sidecar_dir),
        format!("runtime-ready\nsource={source}\n"),
    )
    .map_err(|e| format!("AI 런타임 준비 표시를 저장하지 못했어요: {}", e))
}

#[cfg(target_os = "windows")]
fn windows_runtime_dir_has_required_files(dir: &Path) -> bool {
    if !dir.exists() {
        return false;
    }
    windows_sidecar_required_files()
        .iter()
        .chain(windows_resident_server_required_files().iter())
        .chain(windows_msvc_runtime_files().iter())
        .all(|name| dir.join(name).exists())
}

#[cfg(target_os = "windows")]
fn windows_install_runtime_dirs(install_dir: &Path) -> [PathBuf; 4] {
    [
        install_dir.join(WINDOWS_BUNDLE_SUPPORT_DIR_NAME).join("runtime"),
        install_dir.join(WINDOWS_BUNDLE_SUPPORT_DIR_NAME).join("sidecar"),
        install_dir.join("runtime"),
        install_dir.join("sidecar"),
    ]
}

#[cfg(target_os = "windows")]
fn windows_installed_runtime_dir(install_dir: &Path) -> Option<PathBuf> {
    windows_install_runtime_dirs(install_dir)
        .into_iter()
        .find(|dir| windows_runtime_dir_has_required_files(dir))
}

#[cfg(target_os = "windows")]
fn windows_resource_runtime_dir(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .resolve("runtime", tauri::path::BaseDirectory::Resource)
        .ok()
        .filter(|dir| windows_runtime_dir_has_required_files(dir))
}

#[cfg(target_os = "windows")]
fn windows_install_model_dirs(install_dir: &Path) -> [PathBuf; 4] {
    [
        install_dir.join(WINDOWS_BUNDLE_SUPPORT_DIR_NAME).join("resources").join("models"),
        install_dir.join(WINDOWS_BUNDLE_SUPPORT_DIR_NAME).join("models"),
        install_dir.join("resources").join("models"),
        install_dir.join("models"),
    ]
}

#[cfg(target_os = "windows")]
fn windows_model_dir_has_required_files(dir: &Path) -> bool {
    dir.join("HyperCLOVAX-SEED-Text-Instruct-0.5B-q4_0.gguf").exists()
        && dir.join("hyperclovax_roosy_Q4_K_M.gguf").exists()
}

#[cfg(target_os = "windows")]
fn windows_installed_model_dir(install_dir: &Path) -> Option<PathBuf> {
    windows_install_model_dirs(install_dir)
        .into_iter()
        .find(|dir| windows_model_dir_has_required_files(dir))
}

#[cfg(target_os = "windows")]
fn windows_resource_model_dir(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .resolve("models", tauri::path::BaseDirectory::Resource)
        .ok()
        .filter(|dir| windows_model_dir_has_required_files(dir))
}

#[cfg(target_os = "windows")]
fn windows_runtime_needs_repair(app: &AppHandle) -> bool {
    if windows_resource_runtime_dir(app).is_some() {
        return false;
    }
    let current_exe = match std::env::current_exe() {
        Ok(path) => path,
        Err(_) => return true,
    };
    let install_dir = match current_exe.parent() {
        Some(path) => path,
        None => return true,
    };
    if windows_installed_runtime_dir(install_dir).is_some() {
        return false;
    }
    let Ok(sidecar_dir) = windows_sidecar_storage_dir(app) else {
        return true;
    };
    !windows_runtime_dir_has_required_files(&sidecar_dir) || !windows_runtime_marker_path(&sidecar_dir).exists()
}

#[cfg(target_os = "windows")]
fn find_runtime_llama_server_candidate(root: &Path) -> Option<PathBuf> {
    for candidate in ["llama-server.exe", "llama-server"] {
        if let Some(path) = find_path_recursively(root, &|path| {
            path.is_file() && path.file_name().and_then(|x| x.to_str()) == Some(candidate)
        }) {
            return Some(path);
        }
    }
    None
}

#[cfg(target_os = "windows")]
fn collect_runtime_dlls(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if !root.exists() {
        return out;
    }
    if root.is_file() {
        if root.extension().and_then(|x| x.to_str()).map(|x| x.eq_ignore_ascii_case("dll")) == Some(true) {
            out.push(root.to_path_buf());
        }
        return out;
    }
    let Ok(entries) = fs::read_dir(root) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(collect_runtime_dlls(&path));
        } else if path.extension().and_then(|x| x.to_str()).map(|x| x.eq_ignore_ascii_case("dll")) == Some(true) {
            out.push(path);
        }
    }
    out
}

#[cfg(target_os = "windows")]
fn download_windows_runtime_to_appdata(sidecar_dir: &Path) -> Result<(), String> {
    let temp_root = windows_temp_work_dir("runtime");
    if temp_root.exists() {
        let _ = fs::remove_dir_all(&temp_root);
    }
    fs::create_dir_all(&temp_root).map_err(|e| format!("임시 AI 런타임 폴더를 만들지 못했어요: {}", e))?;
    let zip_path = temp_root.join("runtime.zip");
    let extract_dir = temp_root.join("runtime");

    download_release_zip(WINDOWS_RUNTIME_URL, &zip_path)?;
    extract_release_zip(&zip_path, &extract_dir)?;

    let runtime_server = find_runtime_llama_server_candidate(&extract_dir)
        .ok_or_else(|| "다운로드한 AI 런타임 안에서 llama-server.exe를 찾지 못했어요.".to_string())?;
    let runtime_dlls = collect_runtime_dlls(&extract_dir);

    for required in windows_sidecar_required_files()
        .iter()
        .skip(1)
        .chain(windows_msvc_runtime_files().iter())
    {
        if !runtime_dlls.iter().any(|path| path.file_name().and_then(|x| x.to_str()) == Some(required)) {
            return Err(format!(
                "다운로드한 AI 런타임 안에 필요한 DLL이 빠져 있어요: {}",
                required
            ));
        }
    }

    fs::create_dir_all(sidecar_dir).map_err(|e| format!("공용 AI 런타임 폴더를 만들지 못했어요: {}", e))?;
    let target_server = sidecar_dir.join("llama-server.exe");
    fs::copy(&runtime_server, &target_server)
        .map_err(|e| format!("resident llama-server를 공용 폴더로 복사하지 못했어요: {}", e))?;
    for dll in runtime_dlls {
        let Some(name) = dll.file_name() else {
            continue;
        };
        let target = sidecar_dir.join(name);
        let _ = fs::copy(&dll, &target);
    }
    write_windows_runtime_marker(sidecar_dir, WINDOWS_RUNTIME_URL)?;

    let _ = fs::remove_dir_all(&temp_root);
    Ok(())
}

#[cfg(target_os = "windows")]
fn restore_embedded_windows_runtime_to_appdata(sidecar_dir: &Path) -> Result<bool, String> {
    if EMBEDDED_WINDOWS_RUNTIME_ZIP.is_empty() {
        return Ok(false);
    }

    let temp_root = windows_temp_work_dir("embedded-runtime");
    if temp_root.exists() {
        let _ = fs::remove_dir_all(&temp_root);
    }
    fs::create_dir_all(&temp_root).map_err(|e| format!("내장 runtime 임시 폴더를 만들지 못했어요: {}", e))?;
    let extract_dir = temp_root.join("runtime");

    let cursor = Cursor::new(EMBEDDED_WINDOWS_RUNTIME_ZIP);
    extract_zip_archive_to_dir(cursor, &extract_dir)?;

    let runtime_server = find_runtime_llama_server_candidate(&extract_dir)
        .ok_or_else(|| "실행파일에 포함된 runtime 안에서 llama-server.exe를 찾지 못했어요.".to_string())?;
    let runtime_dlls = collect_runtime_dlls(&extract_dir);

    for required in windows_sidecar_required_files()
        .iter()
        .skip(1)
        .chain(windows_msvc_runtime_files().iter())
    {
        if !runtime_dlls.iter().any(|path| path.file_name().and_then(|x| x.to_str()) == Some(required)) {
            return Err(format!(
                "실행파일에 포함된 runtime 안에 필요한 DLL이 빠져 있어요: {}",
                required
            ));
        }
    }

    fs::create_dir_all(sidecar_dir).map_err(|e| format!("공용 AI 런타임 폴더를 만들지 못했어요: {}", e))?;
    fs::copy(&runtime_server, sidecar_dir.join("llama-server.exe"))
        .map_err(|e| format!("내장 llama-server를 공용 폴더로 복사하지 못했어요: {}", e))?;
    for dll in runtime_dlls {
        let Some(name) = dll.file_name() else {
            continue;
        };
        let _ = fs::copy(&dll, sidecar_dir.join(name));
    }
    write_windows_runtime_marker(sidecar_dir, "embedded-executable")?;

    let _ = fs::remove_dir_all(&temp_root);
    Ok(true)
}

#[cfg(target_os = "windows")]
pub(crate) fn ensure_windows_runtime_cache(app: &AppHandle) -> Result<(), String> {
    let current_exe = std::env::current_exe()
        .map_err(|e| format!("현재 실행 파일 경로를 읽지 못했어요: {}", e))?;
    let install_dir = current_exe
        .parent()
        .ok_or_else(|| "현재 실행 파일 폴더를 찾지 못했어요.".to_string())?;

    let sidecar_dir = windows_sidecar_storage_dir(app)?;
    let resources_dir = windows_resources_storage_dir(app)?;
    fs::create_dir_all(&sidecar_dir).map_err(|e| format!("공용 AI 런타임 폴더를 만들지 못했어요: {}", e))?;
    fs::create_dir_all(&resources_dir).map_err(|e| format!("공용 AI 모델 폴더를 만들지 못했어요: {}", e))?;

    if !windows_runtime_dir_has_required_files(&sidecar_dir) {
        if let Some(resource_runtime) = windows_resource_runtime_dir(app) {
            copy_dir_recursive(&resource_runtime, &sidecar_dir)?;
            if windows_runtime_dir_has_required_files(&sidecar_dir) {
                let _ = write_windows_runtime_marker(&sidecar_dir, "resource-runtime");
            }
        }
    }

    if !windows_runtime_dir_has_required_files(&sidecar_dir) {
        for source in windows_install_runtime_dirs(install_dir) {
            if windows_runtime_dir_has_required_files(&source) {
                copy_dir_recursive(&source, &sidecar_dir)?;
                if windows_runtime_dir_has_required_files(&sidecar_dir) {
                    let _ = write_windows_runtime_marker(&sidecar_dir, &format!("installed-runtime:{}", source.display()));
                }
                break;
            }
        }
    }

    if !windows_runtime_dir_has_required_files(&sidecar_dir) {
        let restored_from_embedded = restore_embedded_windows_runtime_to_appdata(&sidecar_dir)?;
        if !restored_from_embedded && !windows_runtime_dir_has_required_files(&sidecar_dir) {
            download_windows_runtime_to_appdata(&sidecar_dir)?;
        }
    }

    if !windows_runtime_dir_has_required_files(&sidecar_dir) {
        return Err("resident llama-server runtime을 준비하지 못했어요.".to_string());
    }
    if !windows_runtime_marker_path(&sidecar_dir).exists() {
        let _ = write_windows_runtime_marker(&sidecar_dir, "verified-existing-runtime");
    }

    if !windows_model_dir_has_required_files(&resources_dir) {
        if let Some(resource_models) = windows_resource_model_dir(app) {
            copy_dir_recursive(&resource_models, &resources_dir)?;
        }
    }

    let installed_model_dir = windows_installed_model_dir(install_dir);
    if installed_model_dir.is_none() {
        for source in windows_install_model_dirs(install_dir) {
            if source.exists() {
                copy_dir_recursive(&source, &resources_dir)?;
                break;
            }
        }
    }

    if !windows_model_dir_has_required_files(&resources_dir) && installed_model_dir.is_none() {
        return Err(
            "설치된 Windows 앱에서 기본 모델 파일을 찾지 못했어요. installer에 models 리소스가 포함되어야 해요."
                .to_string(),
        );
    }

    Ok(())
}

#[cfg(target_os = "windows")]
fn find_path_recursively(root: &Path, predicate: &dyn Fn(&Path) -> bool) -> Option<PathBuf> {
    if predicate(root) {
        return Some(root.to_path_buf());
    }
    let entries = fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if predicate(&path) {
            return Some(path);
        }
        if path.is_dir() {
            if let Some(found) = find_path_recursively(&path, predicate) {
                return Some(found);
            }
        }
    }
    None
}

#[cfg(target_os = "windows")]
fn validate_extracted_release(extract_dir: &Path) -> Result<(PathBuf, Option<PathBuf>, Option<PathBuf>, Option<PathBuf>), String> {
    let extracted_exe = find_path_recursively(extract_dir, &|path| {
        path.is_file() && path.file_name().and_then(|x| x.to_str()) == Some("roosycozy.exe")
    })
    .ok_or_else(|| "업데이트 압축 파일 안에 roosycozy.exe가 없어요.".to_string())?;

    let extracted_support_dir = find_path_recursively(extract_dir, &|path| {
        path.is_dir() && path.file_name().and_then(|x| x.to_str()) == Some(WINDOWS_BUNDLE_SUPPORT_DIR_NAME)
    })
    .or_else(|| {
        find_path_recursively(extract_dir, &|path| {
            path.is_dir()
                && matches!(
                    path.file_name().and_then(|x| x.to_str()),
                    Some("runtime") | Some("sidecar")
                )
        })
        .and_then(|runtime| runtime.parent().map(|parent| parent.to_path_buf()))
    });

    let Some(extracted_support_dir) = extracted_support_dir else {
        return Ok((extracted_exe, None, None, None));
    };

    let extracted_runtime = if extracted_support_dir.join("runtime").exists() {
        extracted_support_dir.join("runtime")
    } else {
        extracted_support_dir.join("sidecar")
    };
    let extracted_runtime = if extracted_runtime.exists() {
        for required in windows_sidecar_required_files() {
            if !extracted_runtime.join(required).exists() {
                return Err(format!(
                    "업데이트 압축 파일 안에 필요한 resident runtime 파일이 빠져 있어요: {}",
                    required
                ));
            }
        }
        for required in windows_resident_server_required_files() {
            if !extracted_runtime.join(required).exists() {
                return Err(format!(
                    "업데이트 압축 파일 안에 resident 서버 파일이 빠져 있어요: {}",
                    required
                ));
            }
        }
        Some(extracted_runtime)
    } else {
        None
    };

    let extracted_resources = {
        let bundled_models = extracted_support_dir.join("resources").join("models");
        let direct_models = extracted_support_dir.join("models");
        let bundled_resources = extracted_support_dir.join("resources");
        if bundled_models.exists() {
            Some(bundled_models)
        } else if direct_models.exists() {
            Some(direct_models)
        } else if bundled_resources.exists() {
            Some(bundled_resources)
        } else {
            None
        }
    };

    Ok((extracted_exe, extracted_runtime, extracted_resources, Some(extracted_support_dir)))
}

#[cfg(target_os = "windows")]
fn apply_portable_release_update(
    app: &AppHandle,
    asset_url: &str,
    current_exe: &Path,
    replace_exe: bool,
) -> Result<(), String> {
    let temp_root = windows_temp_work_dir("update");
    if temp_root.exists() {
        let _ = fs::remove_dir_all(&temp_root);
    }
    fs::create_dir_all(&temp_root).map_err(|e| format!("임시 업데이트 폴더를 만들지 못했어요: {}", e))?;
    let zip_path = temp_root.join("release.zip");
    let extract_dir = temp_root.join("release");

    download_release_zip(asset_url, &zip_path)?;
    extract_release_zip(&zip_path, &extract_dir)?;

    let sidecar_dir = windows_sidecar_storage_dir(app)?;
    let resources_dir = windows_resources_storage_dir(app)?;
    fs::create_dir_all(&sidecar_dir).map_err(|e| format!("공용 AI 런타임 폴더를 만들지 못했어요: {}", e))?;
    fs::create_dir_all(&resources_dir).map_err(|e| format!("공용 AI 모델 폴더를 만들지 못했어요: {}", e))?;

    let (extracted_exe, extracted_runtime, extracted_resources, _) = validate_extracted_release(&extract_dir)?;

    if let Some(extracted_runtime) = extracted_runtime.as_ref() {
        copy_dir_recursive(extracted_runtime, &sidecar_dir)?;
    }

    if let Some(extracted_resources) = extracted_resources.as_ref() {
        copy_dir_recursive(extracted_resources, &resources_dir)?;
    }

    if replace_exe {
        let staging_dir = windows_staging_dir();
        fs::create_dir_all(&staging_dir)
            .map_err(|e| format!("업데이트 staging 폴더를 만들지 못했어요: {}", e))?;
        let staged_exe = staging_dir.join("roosycozy.exe.new");
        if staged_exe.exists() {
            let _ = fs::remove_file(&staged_exe);
        }
        fs::copy(&extracted_exe, &staged_exe).map_err(|e| format!("새 실행 파일을 준비하지 못했어요: {}", e))?;
        let _ = fs::remove_dir_all(&temp_root);
        schedule_windows_exe_swap(current_exe, &staged_exe)?;
    } else {
        let _ = fs::remove_dir_all(&temp_root);
    }

    Ok(())
}

fn check_and_update_sync(app: AppHandle) -> Result<String, String> {
    #[cfg(not(target_os = "windows"))]
    {
        let _ = app;
        return Ok("윈도우에서만 자동 업데이트를 지원합니다.".to_string());
    }

    #[cfg(target_os = "windows")]
    {
        let app_version = app.package_info().version.to_string();
        let current_exe = std::env::current_exe()
            .map_err(|e| format!("현재 실행 파일 경로를 읽지 못했어요: {}", e))?;

        let latest = latest_github_release()?;
        let latest_version = normalize_release_version(&latest.tag_name);
        let asset = latest
            .assets
            .iter()
            .find(|item| item.name == UPDATE_ASSET_NAME)
            .ok_or_else(|| format!("릴리즈 자산에서 {} 파일을 찾지 못했어요.", UPDATE_ASSET_NAME))?;

        if parse_version_triplet(&app_version) < parse_version_triplet(&latest_version) {
            apply_portable_release_update(
                &app,
                &asset.browser_download_url,
                &current_exe,
                true,
            )?;
            return Ok(format!(
                "업데이트 완료: 버전 {}. 잠시 후 자동으로 새 버전이 다시 열립니다.",
                latest_version
            ));
        }

        ensure_windows_runtime_cache(&app)?;
        let needs_repair = windows_runtime_needs_repair(&app);

        if !needs_repair {
            Ok("최신 버전입니다.".to_string())
        } else {
            apply_portable_release_update(
                &app,
                &asset.browser_download_url,
                &current_exe,
                false,
            )?;
            ensure_windows_runtime_cache(&app)?;
            Ok("프로그램 파일을 복구했어요. resident llama-server runtime을 다시 채워 넣었으니 지금 바로 AI 채팅을 다시 시도해보세요.".to_string())
        }
    }
}

#[command]
async fn check_and_update(app: AppHandle) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || check_and_update_sync(app))
        .await
        .map_err(|e| format!("업데이트 작업 스레드가 중단되었어요: {e}"))?
}

#[command]
fn exit_for_update(app: AppHandle) {
    app.exit(0);
}

#[cfg(target_os = "macos")]
fn shared_state_file_path() -> Result<PathBuf, String> {
    Ok(macos_home_dir()?
        .join("Library")
        .join("Application Support")
        .join("RoosyCozy")
        .join("shared-state-v1.json"))
}

#[cfg(target_os = "windows")]
fn shared_state_file_path() -> Result<PathBuf, String> {
    Ok(windows_appdata_root(CURRENT_WINDOWS_APP_ID).join("shared-state-v1.json"))
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn shared_state_file_path() -> Result<PathBuf, String> {
    std::env::current_dir()
        .map(|dir| dir.join("shared-state-v1.json"))
        .map_err(|e| format!("공용 상태 파일 경로를 찾지 못했어요: {}", e))
}

#[command]
fn load_shared_app_state() -> Result<Option<String>, String> {
    #[cfg(target_os = "macos")]
    {
        let _ = bootstrap_shared_state_from_macos_storage();
    }
    #[cfg(target_os = "windows")]
    {
        let _ = bootstrap_shared_state_from_windows_storage();
    }
    let path = shared_state_file_path()?;
    if !path.exists() {
        return Ok(None);
    }
    match fs::read_to_string(&path) {
        Ok(value) => Ok(Some(value)),
        Err(_) => {
            #[cfg(target_os = "macos")]
            {
                let bytes =
                    fs::read(&path).map_err(|e| format!("공용 상태 파일을 읽지 못했어요: {}", e))?;
                if let Some(value) = decode_storage_text_bytes(&bytes) {
                    let _ = save_shared_app_state(value.clone());
                    return Ok(Some(value));
                }
            }
            Err("공용 상태 파일을 읽지 못했어요.".to_string())
        }
    }
}

#[command]
fn save_shared_app_state(value: String) -> Result<(), String> {
    let path = shared_state_file_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("공용 상태 폴더를 만들지 못했어요: {}", e))?;
    }
    fs::write(&path, value).map_err(|e| format!("공용 상태 파일을 저장하지 못했어요: {}", e))
}

#[command]
fn remove_shared_app_state() -> Result<(), String> {
    let path = shared_state_file_path()?;
    if !path.exists() {
        return Ok(());
    }
    fs::remove_file(&path).map_err(|e| format!("공용 상태 파일을 삭제하지 못했어요: {}", e))
}

fn cleanup_old_versions() {
    if let Ok(current_exe) = std::env::current_exe() {
        let old_exe = current_exe.with_extension("exe.old");
        if old_exe.exists() {
            let _ = fs::remove_file(old_exe);
        }
    }
}

#[cfg(target_os = "macos")]
fn macos_home_dir() -> Result<PathBuf, String> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| "HOME 경로를 찾지 못했어요.".to_string())
}

#[cfg(target_os = "macos")]
fn macos_app_support_dir(app_id: &str) -> Result<PathBuf, String> {
    Ok(macos_home_dir()?
        .join("Library")
        .join("Application Support")
        .join(app_id))
}

#[cfg(target_os = "macos")]
fn macos_dev_webkit_dir() -> Result<PathBuf, String> {
    Ok(macos_home_dir()?
        .join("Library")
        .join("WebKit")
        .join("roosycozy"))
}

#[cfg(target_os = "macos")]
fn macos_webkit_dir(app_id: &str) -> Result<PathBuf, String> {
    Ok(macos_home_dir()?
        .join("Library")
        .join("WebKit")
        .join(app_id))
}

#[cfg(target_os = "macos")]
fn copy_dir_recursive_if_missing(source: &Path, target: &Path) -> Result<(), String> {
    if !source.exists() {
        return Ok(());
    }
    fs::create_dir_all(target).map_err(|e| format!("마이그레이션 폴더를 만들지 못했어요: {}", e))?;
    for entry in fs::read_dir(source).map_err(|e| format!("마이그레이션 폴더를 읽지 못했어요: {}", e))? {
        let entry = entry.map_err(|e| format!("마이그레이션 항목을 읽지 못했어요: {}", e))?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        if source_path.is_dir() {
            copy_dir_recursive_if_missing(&source_path, &target_path)?;
            continue;
        }
        if target_path.exists() {
            continue;
        }
        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("마이그레이션 대상 폴더를 만들지 못했어요: {}", e))?;
        }
        fs::copy(&source_path, &target_path)
            .map_err(|e| format!("마이그레이션 파일 복사에 실패했어요: {}", e))?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn copy_dir_recursive_overwrite(source: &Path, target: &Path) -> Result<(), String> {
    if !source.exists() {
        return Ok(());
    }
    if target.exists() {
        fs::remove_dir_all(target)
            .map_err(|e| format!("기존 저장소 폴더를 비우지 못했어요: {}", e))?;
    }
    fs::create_dir_all(target).map_err(|e| format!("저장소 폴더를 만들지 못했어요: {}", e))?;
    for entry in fs::read_dir(source).map_err(|e| format!("저장소 폴더를 읽지 못했어요: {}", e))? {
        let entry = entry.map_err(|e| format!("저장소 항목을 읽지 못했어요: {}", e))?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        if source_path.is_dir() {
            copy_dir_recursive_overwrite(&source_path, &target_path)?;
            continue;
        }
        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("저장소 대상 폴더를 만들지 못했어요: {}", e))?;
        }
        fs::copy(&source_path, &target_path)
            .map_err(|e| format!("저장소 파일 복사에 실패했어요: {}", e))?;
    }
    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn find_named_dir(root: &Path, target_name: &str, depth: usize) -> Option<PathBuf> {
    if depth == 0 || !root.exists() {
        return None;
    }
    for entry in fs::read_dir(root).ok()? {
        let entry = entry.ok()?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if entry.file_name().to_string_lossy() == target_name {
            return Some(path);
        }
        if let Some(found) = find_named_dir(&path, target_name, depth - 1) {
            return Some(found);
        }
    }
    None
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn collect_named_files(root: &Path, target_name: &str, depth: usize, out: &mut Vec<PathBuf>) {
    if depth == 0 || !root.exists() {
        return;
    }
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_named_files(&path, target_name, depth - 1, out);
            continue;
        }
        if entry.file_name().to_string_lossy() == target_name {
            out.push(path);
        }
    }
}

#[cfg(target_os = "macos")]
fn decode_hex_string(value: &str) -> Option<Vec<u8>> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(trimmed.len() / 2);
    let bytes = trimmed.as_bytes();
    for idx in (0..bytes.len()).step_by(2) {
        let hi = (bytes[idx] as char).to_digit(16)?;
        let lo = (bytes[idx + 1] as char).to_digit(16)?;
        out.push(((hi << 4) | lo) as u8);
    }
    Some(out)
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn decode_storage_text_bytes(bytes: &[u8]) -> Option<String> {
    if let Ok(text) = String::from_utf8(bytes.to_vec()) {
        let normalized = text.trim_matches('\u{feff}').to_string();
        if !normalized.trim().is_empty() {
            return Some(normalized);
        }
    }

    if bytes.len() % 2 != 0 {
        return None;
    }
    let utf16: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect();
    let text = String::from_utf16(&utf16).ok()?;
    let normalized = text.trim_matches('\u{feff}').to_string();
    if normalized.trim().is_empty() {
        None
    } else {
        Some(normalized)
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn score_state_payload(value: &str) -> usize {
    let parsed: serde_json::Value = match serde_json::from_str(value) {
        Ok(parsed) => parsed,
        Err(_) => return 0,
    };
    let count = |key: &str| {
        parsed
            .get(key)
            .and_then(|value| value.as_array())
            .map(|items| items.len())
            .unwrap_or(0)
    };
    let records = count("records");
    let strategy_threads = count("strategyThreadPackages");
    let class_roster = count("classRoster");
    let relationship_groups = count("relationshipGroups");
    let cases = parsed
        .get("cases")
        .and_then(|value| value.as_object())
        .map(|items| items.len())
        .unwrap_or(0);

    records * 10_000
        + cases * 5_000
        + strategy_threads * 2_000
        + class_roster * 500
        + relationship_groups * 500
        + value.len()
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn read_state_payload_from_sqlite(path: &Path) -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        let bytes = fs::read(path).ok()?;
        return extract_state_payload_from_bytes(&bytes);
    }

    #[cfg(target_os = "macos")]
    {
    let output = Command::new("/usr/bin/sqlite3")
        .arg(path)
        .arg("select hex(value) from ItemTable where key='roosycozy_state_v1' limit 1;")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let hex = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let bytes = decode_hex_string(&hex)?;
    decode_storage_text_bytes(&bytes)
    }
}

#[cfg(target_os = "windows")]
fn extract_state_payload_from_text(text: &str) -> Option<String> {
    let trimmed = text.trim_matches('\u{feff}').trim();
    if score_state_payload(trimmed) > 0 {
        return Some(trimmed.to_string());
    }

    const STATE_KEY: &str = "roosycozy_state_v1";
    let mut offset = 0usize;
    while let Some(found) = text[offset..].find(STATE_KEY) {
        let key_idx = offset + found + STATE_KEY.len();
        let Some(rel_start) = text[key_idx..].find('{') else {
            break;
        };
        let start = key_idx + rel_start;
        let mut depth = 0usize;
        let mut in_string = false;
        let mut escaped = false;
        for (idx, ch) in text[start..].char_indices() {
            if in_string {
                if escaped {
                    escaped = false;
                    continue;
                }
                match ch {
                    '\\' => escaped = true,
                    '"' => in_string = false,
                    _ => {}
                }
                continue;
            }

            match ch {
                '"' => in_string = true,
                '{' => depth = depth.saturating_add(1),
                '}' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        let candidate = &text[start..start + idx + ch.len_utf8()];
                        if score_state_payload(candidate) > 0 {
                            return Some(candidate.to_string());
                        }
                        break;
                    }
                }
                _ => {}
            }
        }
        offset = key_idx;
    }
    None
}

#[cfg(target_os = "windows")]
fn extract_state_payload_from_bytes(bytes: &[u8]) -> Option<String> {
    if let Some(decoded) = decode_storage_text_bytes(bytes) {
        if let Some(payload) = extract_state_payload_from_text(&decoded) {
            return Some(payload);
        }
    }

    let lossy = String::from_utf8_lossy(bytes);
    extract_state_payload_from_text(&lossy)
}

#[cfg(target_os = "windows")]
fn should_scan_windows_state_file(path: &Path) -> bool {
    let file_name = path.file_name().and_then(|value| value.to_str()).unwrap_or_default();
    let lower_name = file_name.to_ascii_lowercase();
    if file_name.eq_ignore_ascii_case("shared-state-v1.json")
        || file_name.eq_ignore_ascii_case("localstorage.sqlite3")
        || lower_name == "localstorage.sqlite3-wal"
        || lower_name == "localstorage.sqlite3-shm"
        || file_name.eq_ignore_ascii_case("CURRENT")
        || file_name.starts_with("MANIFEST-")
    {
        return true;
    }
    match path.extension().and_then(|value| value.to_str()).map(|value| value.to_ascii_lowercase()) {
        Some(ext)
            if matches!(
                ext.as_str(),
                "json" | "sqlite" | "sqlite3" | "ldb" | "log" | "wal" | "shm"
            ) =>
        {
            true
        }
        _ => false,
    }
}

#[cfg(target_os = "windows")]
fn collect_state_candidates_from_windows_root(
    label: &str,
    root: &Path,
    priority: usize,
    out: &mut Vec<(usize, usize, String, String)>,
) {
    let mut files = Vec::new();
    collect_named_files(root, "shared-state-v1.json", 6, &mut files);
    collect_named_files(root, "localstorage.sqlite3", 6, &mut files);

    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        if depth > 6 || !dir.exists() {
            continue;
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push((path, depth + 1));
                continue;
            }
            if should_scan_windows_state_file(&path) && !files.iter().any(|existing| existing == &path) {
                files.push(path);
            }
        }
    }

    for file_path in files {
        let payload = if file_path
            .file_name()
            .and_then(|value| value.to_str())
            .map(|value| value.eq_ignore_ascii_case("localstorage.sqlite3"))
            .unwrap_or(false)
        {
            read_state_payload_from_sqlite(&file_path).or_else(|| {
                fs::read(&file_path)
                    .ok()
                    .and_then(|bytes| extract_state_payload_from_bytes(&bytes))
            })
        } else {
            fs::read(&file_path)
                .ok()
                .and_then(|bytes| extract_state_payload_from_bytes(&bytes))
        };
        let Some(payload) = payload else {
            continue;
        };
        let score = score_state_payload(&payload);
        if score == 0 {
            continue;
        }
        out.push((
            score,
            priority,
            format!("{}:{}", label, file_path.display()),
            payload,
        ));
    }
}

#[cfg(target_os = "windows")]
fn windows_env_dir(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).map(PathBuf::from)
}

#[cfg(target_os = "windows")]
fn windows_state_candidate_roots() -> Vec<(usize, String, PathBuf)> {
    let mut out = Vec::<(usize, String, PathBuf)>::new();
    let mut push = |priority: usize, label: String, path: PathBuf| {
        if !path.exists() || out.iter().any(|(_, _, existing)| existing == &path) {
            return;
        }
        out.push((priority, label, path));
    };

    push(30, "appdata:current-shared-root".to_string(), windows_appdata_root(CURRENT_WINDOWS_APP_ID));
    push(29, "appdata:dev".to_string(), windows_appdata_root("roosycozy"));
    push(28, "appdata:legacy".to_string(), windows_appdata_root(LEGACY_WINDOWS_APP_ID));
    push(20, "shared-root".to_string(), windows_shared_root());
    push(19, "legacy-shared-root".to_string(), windows_legacy_shared_root());

    for (env_name, priority) in [("APPDATA", 10usize), ("LOCALAPPDATA", 9usize)] {
        let Some(base) = windows_env_dir(env_name) else {
            continue;
        };
        for (offset, folder) in [
            (4usize, "co.roosycozy.desktop"),
            (3usize, "roosycozy"),
            (2usize, "RoosyCozy"),
            (1usize, "co.roosycozy.app"),
        ] {
            push(
                priority + offset,
                format!("{}:{}", env_name, folder),
                base.join(folder),
            );
        }
    }

    if let Some(user_profile) = windows_env_dir("USERPROFILE") {
        push(
            4,
            "userprofile:Documents/RoosyCozy".to_string(),
            user_profile.join("Documents").join("RoosyCozy"),
        );
    }

    out
}

#[cfg(target_os = "windows")]
pub(crate) fn bootstrap_shared_state_from_windows_storage() -> Result<bool, String> {
    let shared_state_path = shared_state_file_path()?;
    let existing_state = fs::read(&shared_state_path)
        .ok()
        .and_then(|bytes| decode_storage_text_bytes(&bytes))
        .and_then(|value| {
            let trimmed = value.trim().to_string();
            if score_state_payload(&trimmed) > 0 {
                Some(trimmed)
            } else {
                None
            }
        });

    let mut candidates = Vec::new();
    for (priority, label, root) in windows_state_candidate_roots() {
        collect_state_candidates_from_windows_root(&label, &root, priority, &mut candidates);
    }

    let Some((_score, _priority, source, payload)) = candidates
        .into_iter()
        .max_by(|left, right| left.1.cmp(&right.1).then(left.0.cmp(&right.0)))
    else {
        return Ok(false);
    };

    if let Some(existing) = existing_state.as_deref() {
        if existing == payload {
            return Ok(false);
        }
    }

    if let Some(parent) = shared_state_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("공용 상태 폴더를 만들지 못했어요: {}", e))?;
    }
    fs::write(&shared_state_path, payload)
        .map_err(|e| format!("공용 상태 파일을 저장하지 못했어요: {}", e))?;
    eprintln!("shared app state bootstrapped from {}", source);
    Ok(true)
}

#[cfg(target_os = "macos")]
fn collect_state_candidates_from_root(
    label: &str,
    root: &Path,
    priority: usize,
    out: &mut Vec<(usize, usize, String, String)>,
) {
    let mut databases = Vec::new();
    collect_named_files(root, "localstorage.sqlite3", 8, &mut databases);
    for db_path in databases {
        let Some(payload) = read_state_payload_from_sqlite(&db_path) else {
            continue;
        };
        let score = score_state_payload(&payload);
        if score == 0 {
            continue;
        }
        out.push((
            score,
            priority,
            format!("{}:{}", label, db_path.display()),
            payload,
        ));
    }
}

#[cfg(target_os = "macos")]
fn bootstrap_shared_state_from_macos_storage() -> Result<bool, String> {
    let shared_state_path = shared_state_file_path()?;
    let existing_state = fs::read_to_string(&shared_state_path).ok();
    let existing_state = existing_state
        .and_then(|value| decode_storage_text_bytes(value.as_bytes()).or(Some(value)));

    let mut candidates = Vec::new();
    collect_state_candidates_from_root("dev", &macos_dev_webkit_dir()?, 3, &mut candidates);
    collect_state_candidates_from_root(
        "current",
        &macos_webkit_dir(CURRENT_MAC_APP_ID)?,
        2,
        &mut candidates,
    );
    collect_state_candidates_from_root(
        "legacy",
        &macos_webkit_dir(LEGACY_MAC_APP_ID)?,
        1,
        &mut candidates,
    );

    let Some((_score, priority, source, payload)) = candidates
        .into_iter()
        .max_by(|left, right| left.1.cmp(&right.1).then(left.0.cmp(&right.0)))
    else {
        return Ok(false);
    };

    if let Some(existing) = existing_state.as_deref() {
        if existing == payload {
            return Ok(false);
        }
        if priority < 3 && score_state_payload(existing) > 0 {
            return Ok(false);
        }
    }

    if let Some(parent) = shared_state_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("공용 상태 폴더를 만들지 못했어요: {}", e))?;
    }
    fs::write(&shared_state_path, payload)
        .map_err(|e| format!("공용 상태 파일을 저장하지 못했어요: {}", e))?;
    eprintln!("shared app state bootstrapped from {}", source);
    Ok(true)
}

#[cfg(target_os = "macos")]
fn file_contains_bytes(path: &Path, needle: &[u8]) -> bool {
    fs::read(path).map(|bytes| bytes.windows(needle.len()).any(|window| window == needle)).unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn dir_contains_state_key(dir: &Path, state_key: &[u8]) -> bool {
    if !dir.exists() {
        return false;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if dir_contains_state_key(&path, state_key) {
                return true;
            }
            continue;
        }
        if file_contains_bytes(&path, state_key) {
            return true;
        }
    }
    false
}

#[cfg(target_os = "macos")]
fn webkit_root_contains_state_key(root: &Path, state_key: &[u8]) -> bool {
    find_named_dir(root, "LocalStorage", 6)
        .map(|dir| dir_contains_state_key(&dir, state_key))
        .unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn migrate_macos_dev_webkit_storage() -> Result<bool, String> {
    let dev_root = macos_dev_webkit_dir()?;
    let new_root = macos_webkit_dir(CURRENT_MAC_APP_ID)?;
    let state_key = b"roosycozy_state_v1";
    if !webkit_root_contains_state_key(&dev_root, state_key) {
        return Ok(false);
    }
    copy_dir_recursive_overwrite(&dev_root, &new_root)?;
    Ok(true)
}

#[cfg(target_os = "macos")]
fn migrate_macos_legacy_local_storage() -> Result<bool, String> {
    let old_root = macos_webkit_dir(LEGACY_MAC_APP_ID)?;
    let new_root = macos_webkit_dir(CURRENT_MAC_APP_ID)?;
    if !old_root.exists() {
        return Ok(false);
    }

    let state_key = b"roosycozy_state_v1";
    let old_local_storage = find_named_dir(&old_root, "LocalStorage", 6);
    if old_local_storage
        .as_ref()
        .map(|dir| !dir_contains_state_key(dir, state_key))
        .unwrap_or(true)
    {
        return Ok(false);
    }
    let new_local_storage = find_named_dir(&new_root, "LocalStorage", 6);

    match (old_local_storage, new_local_storage) {
        (Some(source_dir), Some(target_dir)) => {
            fs::create_dir_all(&target_dir)
                .map_err(|e| format!("새 LocalStorage 폴더를 준비하지 못했어요: {}", e))?;
            for filename in ["localstorage.sqlite3", "localstorage.sqlite3-shm", "localstorage.sqlite3-wal"] {
                let source_path = source_dir.join(filename);
                if !source_path.exists() {
                    continue;
                }
                let target_path = target_dir.join(filename);
                fs::copy(&source_path, &target_path)
                    .map_err(|e| format!("기존 LocalStorage 파일을 복사하지 못했어요: {}", e))?;
            }
            Ok(true)
        }
        (Some(_), None) => {
            copy_dir_recursive_if_missing(&old_root, &new_root)?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

#[cfg(target_os = "macos")]
fn migrate_macos_legacy_app_support() -> Result<bool, String> {
    let old_dir = macos_app_support_dir(LEGACY_MAC_APP_ID)?;
    let new_dir = macos_app_support_dir(CURRENT_MAC_APP_ID)?;
    if !old_dir.exists() {
        return Ok(false);
    }
    copy_dir_recursive_if_missing(&old_dir, &new_dir)?;
    Ok(true)
}

#[cfg(target_os = "macos")]
fn migrate_macos_legacy_storage() -> Result<(), String> {
    let migrated_dev_storage = migrate_macos_dev_webkit_storage()?;
    let migrated_local_storage = if migrated_dev_storage {
        false
    } else {
        migrate_macos_legacy_local_storage()?
    };
    let migrated_app_support = migrate_macos_legacy_app_support()?;
    let _ = bootstrap_shared_state_from_macos_storage()?;
    let _ = migrated_local_storage || migrated_app_support || migrated_dev_storage;
    Ok(())
}

fn main() {
    cleanup_old_versions();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            #[cfg(target_os = "macos")]
            if let Err(err) = migrate_macos_legacy_storage() {
                eprintln!("legacy macOS storage migration skipped: {}", err);
            }
            #[cfg(target_os = "windows")]
            if let Err(err) = bootstrap_shared_state_from_windows_storage() {
                eprintln!("legacy Windows shared-state bootstrap skipped: {}", err);
            }
            #[cfg(target_os = "windows")]
            if let Err(err) = ensure_windows_runtime_cache(&app.handle().clone()) {
                eprintln!("Windows runtime bootstrap skipped: {}", err);
            }
            #[cfg(target_os = "windows")]
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_decorations(false);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::engine_rank,
            commands::engine_advise,
            commands::strategy_agent_chat,
            commands::strategy_prewarm_backend,
            commands::strategy_model_status,
      commands::start_strategy_model_download,
      commands::download_strategy_models,
            commands::get_device_signer_info,
            commands::sign_integrity_payload,
            commands::verify_integrity_payload,
            commands::export_case_pdf,
            commands::export_backup_json,
            commands::import_backup_json,
            load_shared_app_state,
            save_shared_app_state,
            remove_shared_app_state,
            check_and_update,
            exit_for_update
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
