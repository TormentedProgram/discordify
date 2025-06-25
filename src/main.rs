mod video_transcode;
mod audio_transcode;
mod utils;

use std::{env, fs};
use std::fs::{metadata, File};
use std::io::Read;
use std::path::{Path};
use std::time::Instant;
use tokio;
use sha1::{Sha1, Digest};
use ffmpeg_next as ffmpeg;
use rust_embed::Embed;

const OVERRIDDEN_PATH:&str = "";

#[derive(Embed)]
#[folder = "assets/"]
#[prefix = "assets/"]
struct Asset;

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

    match metadata(&input_file) {
        Ok(meta) => {
            let file_size_bytes = meta.len();
            let video_size = file_size_bytes as f32 / (1024.0 * 1024.0);
            if video_size <= input_size {
                println!("[RUST] File is {video_size} MB which is already below {input_size} MB, so nothing happened!");
                utils::copy_video_file(&input_file);
                return Ok(());
            }
        }
        Err(e) => {
            eprintln!("Error reading file metadata: {}", e);
        }
    }
    let actual_start_time;
    match ffmpeg::init() {
        Ok(_) => {
            println!("FFmpeg initialized successfully.");
            actual_start_time = Instant::now();
        },
        Err(e) => {
            eprintln!("Failed to initialize FFmpeg: {}", e);
            return Err(e.into());
        }
    }

    let audio_output_path = audio_transcode::audio(&input_file, &input_size, actual_start_time).await.unwrap_or_else(|e| None);
    
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

    let audio_output = audio_output_path.clone().unwrap();

    let audio_output_path_str = &audio_output
        .to_str()
        .expect("failed to convert audio output path to string");

    let mut video_size:f32;
    let mut video_output_path;
    let mut additional_shrink_mb = 0.0;
    let mut shrink_ratio;
    
    loop {
        let target_size = input_size - additional_shrink_mb;
        video_output_path = video_transcode::video(input_file.clone(), audio_output_path.clone().expect("idk why it wont clone"), output_path.clone(), &target_size, actual_start_time).await;

        match metadata(&video_output_path) {
            Ok(meta) => {
                let file_size_bytes = meta.len();
                video_size = file_size_bytes as f32 / (1024.0 * 1024.0);
                if video_size <= input_size {
                    println!("[RUST] Video transcoding complete: {} MB", video_size);
                    break;
                } else {
                    println!("[RUST] Video pass failed: wanted {:.2} MB, received {:.2} MB. Starting next pass...", input_size, video_size);
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

    match fs::remove_file(&audio_output_path_str) {
        Ok(_) => {},
        Err(e) => eprintln!("Error removing audio file: {}", e),
    }

    match fs::rename(&video_output_path, &final_output_path) {
        Ok(_) => {},
        Err(e) => eprintln!("Error renaming file: {}", e),
    }

    utils::copy_video_file(&final_output_path);

    let (_stream, stream_handle) = rodio::OutputStream::try_default().unwrap();
    let embedded_file = Asset::get("assets/done.wav").unwrap();
    let cursor = std::io::Cursor::new(embedded_file.data); // Use the correct field to access the data
    let beep1 = stream_handle.play_once(cursor).unwrap();
    beep1.set_volume(0.2);
    beep1.sleep_until_end();
    drop(beep1);

    Ok(())
}
