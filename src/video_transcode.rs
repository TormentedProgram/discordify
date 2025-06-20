use ffmpeg_next as ffmpeg;

//video
use ffmpeg::{
    codec, decoder, encoder, format, frame, log, media, picture, Dictionary, Packet, Rational
};
use std::collections::HashMap;
use std::env;
use std::fs::metadata;
use std::path::PathBuf;
use std::time::Instant;

use crate::OVERRIDDEN_PATH;

struct VideoTranscoder {
    output_stream_index: usize,
    decoder: decoder::Video,
    input_time_base: Rational,
    video_encoder: encoder::Video,
    logging_enabled: bool,
    frame_count: usize,
    last_log_frame_count: usize,
    starting_time: Instant,
    last_log_time: Instant,
}

pub async fn video(input_file: PathBuf, audio_path: PathBuf, output_path: PathBuf, wanted_size:&f32) -> PathBuf {
    let mut audio_file_size = 0.0;
    match metadata(&audio_path) {
        Ok(meta) => {
            let file_size_bytes = meta.len();
            audio_file_size = file_size_bytes as f64 / (1024.0 * 1024.0);
            println!("File size: {audio_file_size}");
        }
        Err(e) => {
            eprintln!("Error getting file metadata: {}", e);
        }
    }

    let output_file = output_path
        .to_str()
        .expect("failed to convert output file path to string");

    log::set_level(log::Level::Info);

    let audio_input_context = format::input(&audio_path).unwrap();

    let mut input_context = format::input(&input_file).unwrap();
    let mut output_context = format::output(&output_file).unwrap();

    format::context::input::dump(&input_context, 0, Some(&input_file.to_str().expect("failed to convert input file path to string")));

    let x264_opts_string = "preset=ultrafast".to_string();
    let x264_opts = parse_opts(x264_opts_string)
        .expect("invalid x264 options string");

    let best_audio_index = audio_input_context
        .streams()
        .best(media::Type::Audio)
        .map(|stream| stream.index());

    let best_video_stream_index = input_context
        .streams()
        .best(media::Type::Video)
        .map(|stream| stream.index());
    let mut stream_mapping: Vec<isize> = vec![0; input_context.nb_streams() as _];
    let mut input_stream_time_bases = vec![Rational(0, 0); input_context.nb_streams() as _];
    let mut output_stream_time_bases = vec![Rational(0, 0); input_context.nb_streams() as _];
    let mut transcoders = HashMap::new();
    let mut output_stream_index = 0;
    for (input_stream_index, input_stream) in input_context.streams().enumerate() {
        let input_stream_medium = input_stream.parameters().medium();
        if input_stream_medium != media::Type::Video && input_stream_medium != media::Type::Audio && input_stream_medium != media::Type::Subtitle
        {
            stream_mapping[input_stream_index] = -1;
            continue;
        }
        stream_mapping[input_stream_index] = output_stream_index;
        input_stream_time_bases[input_stream_index] = input_stream.time_base();

        if input_stream_medium == media::Type::Video {
            // Initialize transcoder for video stream.
            transcoders.insert(
                input_stream_index,
                VideoTranscoder::new(
                    &input_stream,
                    &mut output_context,
                    output_stream_index as _,
                    x264_opts.to_owned(),
                    Some(input_stream_index) == best_video_stream_index,
                    audio_file_size as f32,
                    wanted_size,
                ).unwrap(),
            );
            }else if input_stream_medium == media::Type::Audio {
                if let Some(audio_index) = best_audio_index {
                    let audio_stream = audio_input_context.streams().nth(audio_index).unwrap();
                    let mut output_audio_stream = output_context.add_stream(encoder::find(codec::Id::None)).unwrap();
                    output_audio_stream.set_parameters(audio_stream.parameters());
                    unsafe {
                        (*output_audio_stream.parameters().as_mut_ptr()).codec_tag = 0;
                    }
                }
            } else {
                // Set up for stream copy for non-video stream (if applicable).
                let mut output_stream = output_context.add_stream(encoder::find(codec::Id::None)).unwrap();
                output_stream.set_parameters(input_stream.parameters());
                unsafe {
                    (*output_stream.parameters().as_mut_ptr()).codec_tag = 0;
                }
            }
            output_stream_index += 1;
        }

    output_context.set_metadata(input_context.metadata().to_owned());
    format::context::output::dump(&output_context, 0, Some(&output_file));
    output_context.write_header().unwrap();

    for (output_stream_index, _) in output_context.streams().enumerate() {
        output_stream_time_bases[output_stream_index] = output_context.stream(output_stream_index as _).unwrap().time_base();
    }

    for (stream, mut packet) in input_context.packets() {
        let input_stream_index = stream.index();
        let output_stream_index = stream_mapping[input_stream_index];
        if output_stream_index < 0 {
            continue;
        }
        let output_stream_time_base = output_stream_time_bases[output_stream_index as usize];
        match transcoders.get_mut(&input_stream_index) {
            Some(transcoder) => {
                transcoder.send_packet_to_decoder(&packet);
                transcoder.receive_and_process_decoded_frames(&mut output_context, output_stream_time_base);
            }
            None => {
                // Do stream copy on non-video streams.
                packet.rescale_ts(input_stream_time_bases[input_stream_index], output_stream_time_base);
                packet.set_position(-1);
                packet.set_stream(output_stream_index as _);
                packet.write_interleaved(&mut output_context).unwrap();
            }
        }
    }

    // Flush encoders and decoders.
    for (output_stream_index, transcoder) in transcoders.iter_mut() {
        let output_stream_time_base = output_stream_time_bases[*output_stream_index];
        transcoder.send_eof_to_decoder();
        transcoder.receive_and_process_decoded_frames(&mut output_context, output_stream_time_base);
        transcoder.send_eof_to_encoder();
        transcoder.receive_and_process_encoded_packets(&mut output_context, output_stream_time_base);
    }

    output_context.write_trailer().unwrap();
    output_path
}

impl VideoTranscoder {
    fn new(
        input_stream: &format::stream::Stream,
        output_context: &mut format::context::Output,
        output_stream_index: usize,
        x264_opts: Dictionary,
        enable_logging: bool,
        audio_file_size: f32,
        wanted_size: &f32,
    ) -> Result<Self, ffmpeg::Error> {
        let global_header = output_context.format().flags().contains(format::Flags::GLOBAL_HEADER);
        let decoder = ffmpeg::codec::context::Context::from_parameters(input_stream.parameters())?
            .decoder()
            .video()?;

        let codec = encoder::find(codec::Id::H264);
        let mut output_stream = output_context.add_stream(codec)?;

        let input_file:String;
        if OVERRIDDEN_PATH.is_empty() {
            input_file = env::args().nth(1).expect("missing input file");
        } else {
            input_file = OVERRIDDEN_PATH.to_string();
        }
        let input_context = format::input(&input_file)?;

        let mut video_encoder = codec::context::Context::new_with_codec(codec.ok_or(ffmpeg::Error::InvalidData)?)
            .encoder()
            .video()?;

        output_stream.set_parameters(&video_encoder);
        video_encoder.set_height(decoder.height());
        video_encoder.set_width(decoder.width());
        video_encoder.set_aspect_ratio(decoder.aspect_ratio());
        video_encoder.set_format(decoder.format());
        video_encoder.set_frame_rate(decoder.frame_rate());
        video_encoder.set_time_base(input_stream.time_base());
        let wanted_mb:f64 = (wanted_size - audio_file_size) as f64;
        let wanted_bytes:f64 = wanted_mb * 1024.0 * 1024.0;
        let wanted_bits:f64 = wanted_bytes * 8.0;
        let duration_secs = input_context.duration() as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE);
        let cool_bit_rate:f64 = wanted_bits / duration_secs;

        video_encoder.set_bit_rate(cool_bit_rate as usize);
        video_encoder.set_max_bit_rate(cool_bit_rate as usize);

        if global_header {
            video_encoder.set_flags(codec::Flags::GLOBAL_HEADER);
        }

        let opened_video_encoder = video_encoder
            .open_with(x264_opts)
            .expect("error opening x264 with supplied settings");


        output_stream.set_parameters(&opened_video_encoder);
        Ok(Self {
            output_stream_index,
            decoder,
            input_time_base: input_stream.time_base(),
            video_encoder: opened_video_encoder,
            logging_enabled: enable_logging,
            frame_count: 0,
            last_log_frame_count: 0,
            starting_time: Instant::now(),
            last_log_time: Instant::now(),
        })
    }

    fn send_packet_to_decoder(&mut self, packet: &Packet) {
        self.decoder.send_packet(packet).unwrap();
    }

    fn send_eof_to_decoder(&mut self) {
        self.decoder.send_eof().unwrap();
    }

    fn receive_and_process_decoded_frames(
        &mut self,
        output_context: &mut format::context::Output,
        output_stream_time_base: Rational,
    ) {
        let mut frame = frame::Video::empty();
        while self.decoder.receive_frame(&mut frame).is_ok() {
            self.frame_count += 1;
            let timestamp = frame.timestamp();
            self.log_progress(f64::from(
                Rational(timestamp.unwrap_or(0) as i32, 1) * self.input_time_base,
            ));
            frame.set_pts(timestamp);
            frame.set_kind(picture::Type::None);
            self.send_frame_to_encoder(&frame);
            self.receive_and_process_encoded_packets(output_context, output_stream_time_base);
        }
    }

    fn send_frame_to_encoder(&mut self, frame: &frame::Video) {
        self.video_encoder.send_frame(frame).unwrap();
    }

    fn send_eof_to_encoder(&mut self) {
        self.video_encoder.send_eof().unwrap();
    }

    fn receive_and_process_encoded_packets(
        &mut self,
        output_context: &mut format::context::Output,
        output_stream_time_base: Rational,
    ) {
        let mut encoded = Packet::empty();
        while self.video_encoder.receive_packet(&mut encoded).is_ok() {
            encoded.set_stream(self.output_stream_index);
            encoded.rescale_ts(self.input_time_base, output_stream_time_base);
            encoded.write_interleaved(output_context).unwrap();
        }
    }

    fn log_progress(&mut self, timestamp: f64) {
        if !self.logging_enabled
            || (self.frame_count - self.last_log_frame_count < 100
            && self.last_log_time.elapsed().as_secs_f64() < 1.0)
        {
            return;
        }

        let total_seconds = self.starting_time.elapsed().as_secs_f64() as u64;
        let hours = total_seconds / 3600;
        let minutes = (total_seconds % 3600) / 60;
        let seconds = total_seconds % 60;
        let formatted_time = format!("{:02}:{:02}:{:02}", hours, minutes, seconds);
        eprintln!(
            "[RUST] VIDEO ELAPSED: \t{:8.2}s\tFRAMES: {:8}\tTIMESTAMP: \t{formatted_time}",
            self.starting_time.elapsed().as_secs_f64(),
            self.frame_count,
        );
        self.last_log_frame_count = self.frame_count;
        self.last_log_time = Instant::now();
    }
}

fn parse_opts<'a>(s: String) -> Option<Dictionary<'a>> {
    let mut dict = Dictionary::new();
    for keyval in s.split_terminator(',') {
        let tokens: Vec<&str> = keyval.split('=').collect();
        match tokens[..] {
            [key, val] => dict.set(key, val),
            _ => return None,
        }
    }
    Some(dict)
}