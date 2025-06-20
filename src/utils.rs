use std::io::BufReader;
use std::path::PathBuf;
use std::process::{Command, Stdio};

pub fn copy_video_file(file_path:&PathBuf) {
    let video_file_path = file_path.to_str().unwrap().to_string();

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

    let (_stream, stream_handle) = rodio::OutputStream::try_default().unwrap();
    let file = std::fs::File::open("assets/done.wav").unwrap();
    let beep1 = stream_handle.play_once(BufReader::new(file)).unwrap();
    beep1.set_volume(0.2);
    beep1.sleep_until_end();
    drop(beep1);
}