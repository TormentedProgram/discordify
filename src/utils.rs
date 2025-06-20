use std::{env, fs};
use std::path::PathBuf;
use std::process::{Command};

fn is_wlcopy_available() -> bool {
    Command::new("wlcopy").arg("-version").output().map_or(false, |output| output.status.success())
}

fn is_wayland() -> bool {
    env::var("WAYLAND_DISPLAY").is_ok()
}

pub fn copy_video_file(file_path:&PathBuf) {
    #[cfg(not(target_os = "linux"))]
    {
        eprintln!("Copying video-file feature is only supported on Linux systems.. Continuing without it..");
        return;
    }
    #[cfg(target_os = "linux")]
    {
        if !is_wayland() {
            eprintln!("Copying video-file feature is only supported on Wayland systems.. Continuing without it..");
            return;
        }
        if !is_wlcopy_available() {
            eprintln!("wlcopy is not available. Please install wl-clipboard.. Continuing without it..");
            return;
        }
    
        match fs::canonicalize(&file_path) {
            Ok(absolute_path) => {
                let video_file_path = absolute_path.to_str().unwrap().to_string();

                let command = format!("echo -n 'file://{}' | wl-copy -t text/uri-list", video_file_path);

                let status = Command::new("sh")
                    .arg("-c")
                    .arg(command)
                    .status()
                    .expect("Failed to execute command");

                if !status.success() {
                    eprintln!("Command failed with status: {}", status);
                } else {
                    println!("Video successfully copied to clipboard..");
                }
            }
            Err(_) => {
                eprintln!("Failed to get absolute path for the video file: {}", file_path.display());
            }
        }
    }
}