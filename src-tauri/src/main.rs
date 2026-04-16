#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod engine;
mod commands;

use std::fs;
#[cfg(target_os = "windows")]
use std::fs::File;
#[cfg(target_os = "windows")]
use std::io;
#[cfg(target_os = "windows")]
use std::path::Path;
#[cfg(target_os = "windows")]
use std::process::Command;
#[cfg(target_os = "windows")]
use tauri::Manager;
use tauri::{command, AppHandle};

#[cfg(target_os = "windows")]
const UPDATE_REPO_OWNER: &str = "myongsung";
#[cfg(target_os = "windows")]
const UPDATE_REPO_NAME: &str = "roosycozy";
#[cfg(target_os = "windows")]
const UPDATE_ASSET_NAME: &str = "roosycozy-x86_64-pc-windows-msvc.zip";

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
    let current_dir = current_exe
        .parent()
        .ok_or_else(|| "현재 실행 파일 폴더를 찾지 못했어요.".to_string())?;
    let current_name = current_exe
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "현재 실행 파일 이름을 읽지 못했어요.".to_string())?;
    let staged_name = staged_exe
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "새 실행 파일 이름을 읽지 못했어요.".to_string())?;
    let script_path = current_dir.join("roosycozy-update.cmd");
    let pid = std::process::id();
    let script = format!(
        "@echo off\r\n\
setlocal\r\n\
set \"TARGET_DIR={target_dir}\"\r\n\
set \"CURRENT_EXE={current_name}\"\r\n\
set \"STAGED_EXE={staged_name}\"\r\n\
set \"PID={pid}\"\r\n\
:waitloop\r\n\
tasklist /FI \"PID eq %PID%\" | findstr /R /C:\"\\<%PID%\\>\" >nul\r\n\
if %ERRORLEVEL%==0 (\r\n\
  timeout /t 1 /nobreak >nul\r\n\
  goto waitloop\r\n\
)\r\n\
if exist \"%TARGET_DIR%\\%CURRENT_EXE%.old\" del /f /q \"%TARGET_DIR%\\%CURRENT_EXE%.old\"\r\n\
if exist \"%TARGET_DIR%\\%CURRENT_EXE%\" move /Y \"%TARGET_DIR%\\%CURRENT_EXE%\" \"%TARGET_DIR%\\%CURRENT_EXE%.old\" >nul\r\n\
if exist \"%TARGET_DIR%\\%STAGED_EXE%\" move /Y \"%TARGET_DIR%\\%STAGED_EXE%\" \"%TARGET_DIR%\\%CURRENT_EXE%\" >nul\r\n\
del /f /q \"%~f0\"\r\n\
endlocal\r\n",
        target_dir = current_dir.display(),
        current_name = current_name,
        staged_name = staged_name,
        pid = pid
    );
    fs::write(&script_path, script).map_err(|e| format!("업데이트 교체 스크립트를 만들지 못했어요: {}", e))?;
    Command::new("cmd")
        .args(["/C", "start", "", "/MIN", script_path.to_string_lossy().as_ref()])
        .spawn()
        .map_err(|e| format!("업데이트 적용 스크립트를 실행하지 못했어요: {}", e))?;
    Ok(())
}

#[command]
fn check_and_update(app: AppHandle) -> Result<String, String> {
    #[cfg(not(target_os = "windows"))]
    {
        let _ = app;
        return Ok("윈도우에서만 자동 업데이트를 지원합니다.".to_string());
    }

    #[cfg(target_os = "windows")]
    {
        let app_version = app.package_info().version.to_string();
        let latest = latest_github_release()?;
        let latest_version = normalize_release_version(&latest.tag_name);
        if parse_version_triplet(&app_version) >= parse_version_triplet(&latest_version) {
            Ok("최신 버전입니다.".to_string())
        } else {
            let asset = latest
                .assets
                .iter()
                .find(|item| item.name == UPDATE_ASSET_NAME)
                .ok_or_else(|| format!("릴리즈 자산에서 {} 파일을 찾지 못했어요.", UPDATE_ASSET_NAME))?;

            let current_exe = std::env::current_exe()
                .map_err(|e| format!("현재 실행 파일 경로를 읽지 못했어요: {}", e))?;
            let install_dir = current_exe
                .parent()
                .ok_or_else(|| "현재 실행 파일 폴더를 찾지 못했어요.".to_string())?
                .to_path_buf();

            let temp_root = std::env::temp_dir().join(format!("roosycozy-update-{}", std::process::id()));
            if temp_root.exists() {
                let _ = fs::remove_dir_all(&temp_root);
            }
            fs::create_dir_all(&temp_root).map_err(|e| format!("임시 업데이트 폴더를 만들지 못했어요: {}", e))?;
            let zip_path = temp_root.join("release.zip");
            let extract_dir = temp_root.join("release");

            download_release_zip(&asset.browser_download_url, &zip_path)?;
            extract_release_zip(&zip_path, &extract_dir)?;

            let extracted_exe = extract_dir.join("roosycozy.exe");
            if !extracted_exe.exists() {
                return Err("업데이트 압축 파일 안에 roosycozy.exe가 없어요.".to_string());
            }

            let extracted_sidecar = extract_dir.join("sidecar");
            let extracted_resources = extract_dir.join("resources");
            if extracted_sidecar.exists() {
                copy_dir_recursive(&extracted_sidecar, &install_dir.join("sidecar"))?;
            }
            if extracted_resources.exists() {
                copy_dir_recursive(&extracted_resources, &install_dir.join("resources"))?;
            }

            let staged_exe = install_dir.join("roosycozy.exe.new");
            if staged_exe.exists() {
                let _ = fs::remove_file(&staged_exe);
            }
            fs::copy(&extracted_exe, &staged_exe).map_err(|e| format!("새 실행 파일을 준비하지 못했어요: {}", e))?;
            let _ = fs::remove_dir_all(&temp_root);
            schedule_windows_exe_swap(&current_exe, &staged_exe)?;

            Ok(format!(
                "업데이트 완료: 버전 {}. 앱을 종료하면 새 버전과 sidecar가 함께 적용됩니다.",
                latest_version
            ))
        }
    }
}

fn cleanup_old_versions() {
    if let Ok(current_exe) = std::env::current_exe() {
        let old_exe = current_exe.with_extension("exe.old");
        if old_exe.exists() {
            let _ = fs::remove_file(old_exe);
        }
    }
}

fn main() {
    cleanup_old_versions();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|_app| {
            #[cfg(target_os = "windows")]
            if let Some(window) = _app.get_webview_window("main") {
                let _ = window.set_decorations(false);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::engine_rank,
            commands::engine_advise,
            commands::engine_classify_risk,
            commands::strategy_agent_chat,
            commands::strategy_model_status,
      commands::start_strategy_model_download,
      commands::download_strategy_models,
            commands::get_device_signer_info,
            commands::sign_integrity_payload,
            commands::verify_integrity_payload,
            commands::export_case_pdf,
            commands::export_backup_json,
            commands::import_backup_json,
            check_and_update
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
