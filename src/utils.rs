use std::fs;
use std::path::PathBuf;
use std::process::{Command};
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "assets/"]
#[prefix = "assets/"]
struct Asset;

pub fn copy_video_file(file_path:&PathBuf) {
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
            }else{
                println!("Video successfully copied to clipboard..");
            }
        }
        Err(e) => {
            eprintln!("Error converting to absolute path: {}", e);
        }
    }

    let (_stream, stream_handle) = rodio::OutputStream::try_default().unwrap();
    let embedded_file = Asset::get("assets/done.wav").unwrap();
    let cursor = std::io::Cursor::new(embedded_file.data); // Use the correct field to access the data
    let beep1 = stream_handle.play_once(cursor).unwrap();
    beep1.set_volume(0.2);
    beep1.sleep_until_end();
    drop(beep1);
}