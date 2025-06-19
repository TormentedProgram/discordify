mod video_transcode;
mod audio_transcode;

use std::{env, fs};
use std::fs::{metadata, File};
use std::io::Read;
use std::path::Path;
use std::process::Command;
use tokio;
use wl_clipboard_rs::copy::{MimeType, Options, Source};
use sha1::{Sha1, Digest};
const OVERRIDDEN_PATH:&str = "";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let input_file: String;
    let input_size: f32;
    if OVERRIDDEN_PATH.is_empty() {
        input_file = env::args().nth(1).expect("missing input file");
    } else {
        input_file = OVERRIDDEN_PATH.to_string();
    }

    input_size = env::args().nth(2).expect("missing set file-size").parse().expect("unable to parse input size");

    let input_file = Path::new(&input_file).to_path_buf();
    let audio_output_path = audio_transcode::audio(&input_file).await;

    let input_file_name = input_file
        .file_name()
        .and_then(|s| s.to_str())
        .expect("invalid input file name");

    let mut hasher = Sha1::new();
    let mut file = File::open(&input_file).unwrap();

    let mut buffer = [0; 1024];
    loop {
        let bytes_read = file.read(&mut buffer).unwrap();
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }
    let result = hasher.finalize();
    let input_file_name = result.iter().map(|byte| format!("{:02x}", byte)).collect::<String>();

    let output_path = input_file
        .parent()
        .expect("input file must have a parent directory")
        .join(input_file_name)
        .with_extension("mp4");

    let final_output_path = input_file
        .parent()
        .expect("input file must have a parent directory")
        .join("discord_ready_video")
        .with_extension("mp4");

    let audio_output = audio_output_path.clone();

    let audio_output_path_str = audio_output
        .to_str()
        .expect("failed to convert audio output path to string");

    let mut video_size:f32;
    let mut video_output_path;
    let mut additional_shrink_mb = 0.0;
    let mut shrink_ratio;
    
    loop {
        let target_size = input_size - additional_shrink_mb;
        video_output_path = video_transcode::video(input_file.clone(), audio_output_path.clone(), output_path.clone(), &target_size).await;

        match metadata(&video_output_path) {
            Ok(meta) => {
                let file_size_bytes = meta.len();
                video_size = file_size_bytes as f32 / (1024.0 * 1024.0);
                if video_size <= input_size {
                    println!("Video transcoding complete: {} MB", video_size);
                    break;
                } else {
                    println!("Video pass-size mismatch: expected {:.2} MB, got {:.2} MB. Retrying...", input_size, video_size);
                    shrink_ratio = video_size / 25.0;
                    additional_shrink_mb += shrink_ratio;
                }
            }
            Err(e) => {
                eprintln!("Error getting file metadata: {}", e);
                break;
            }
        }
    }

    let video_output_path_str = video_output_path
        .to_str()
        .expect("failed to convert video output path to string");

    match std::fs::remove_file(&audio_output_path_str) {
        Ok(_) => {},
        Err(e) => eprintln!("Error removing audio file: {}", e),
    }

    match std::fs::rename(&video_output_path, &final_output_path) {
        Ok(_) => {},
        Err(e) => eprintln!("Error renaming file: {}", e),
    }

    let opts = Options::new();
    opts.copy(Source::Bytes(video_output_path_str.to_string().into_bytes().into()), MimeType::Autodetect)?;
    println!("Copied output path to clipboard!");

    Ok(())
}
