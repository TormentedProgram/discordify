use std::fs::File;
use std::io::Read;
use ffmpeg_next as ffmpeg;

use std::path::{Path, PathBuf};
use std::time::Instant;
use ffmpeg::{codec, filter, frame, media};
use ffmpeg_next::{format, Rational};
use sha1::{Sha1, Digest};

fn filter(
    spec: &str,
    decoder: &codec::decoder::Audio,
    encoder: &codec::encoder::Audio,
) -> Result<filter::Graph, ffmpeg::Error> {
    let mut filter = filter::Graph::new();

    let args = format!(
        "time_base={}:sample_rate={}:sample_fmt={}:channel_layout=0x{:x}",
        decoder.time_base(),
        decoder.rate(),
        decoder.format().name(),
        decoder.channel_layout().bits()
    );

    filter.add(&filter::find("abuffer").unwrap(), "in", &args)?;
    filter.add(&filter::find("abuffersink").unwrap(), "out", "")?;

    {
        let mut out = filter.get("out").unwrap();

        out.set_sample_format(encoder.format());
        out.set_channel_layout(encoder.channel_layout());
        out.set_sample_rate(encoder.rate());
    }

    filter.output("in", 0)?.input("out", 0)?.parse(spec)?;
    filter.validate()?;

    println!("{}", filter.dump());

    if let Some(codec) = encoder.codec() {
        if !codec
            .capabilities()
            .contains(ffmpeg::codec::capabilities::Capabilities::VARIABLE_FRAME_SIZE)
        {
            filter
                .get("out")
                .unwrap()
                .sink()
                .set_frame_size(encoder.frame_size());
        }
    }

    Ok(filter)
}

struct Transcoder {
    stream: usize,
    filter: filter::Graph,
    decoder: codec::decoder::Audio,
    encoder: codec::encoder::Audio,
    in_time_base: ffmpeg::Rational,
    out_time_base: ffmpeg::Rational,
    input_time_base: Rational,
    last_log_time: Instant,
    last_log_frame_count: usize,
    starting_time: Instant,
    frame_count: usize
}

fn linear_scale(input: f32, input_min: f32, input_max: f32, output_min: f32, output_max: f32) -> f32 {
    (input - input_min) / (input_max - input_min) * (output_max - output_min) + output_min
}

fn transcoder<P: AsRef<Path> + ?Sized>(
    ictx: &mut format::context::Input,
    octx: &mut format::context::Output,
    path: &P,
    filter_spec: &str,
    file_size: &f32,
) -> Result<Transcoder, ffmpeg::Error> {
    let input = ictx
        .streams()
        .best(media::Type::Audio)
        .expect("could not find best audio stream");
    let context = ffmpeg::codec::context::Context::from_parameters(input.parameters())?;
    let mut decoder = context.decoder().audio()?;
    let codec = ffmpeg::encoder::find(octx.format().codec(path, media::Type::Audio))
        .expect("failed to find encoder")
        .audio()?;
    let global = octx
        .format()
        .flags()
        .contains(ffmpeg::format::flag::Flags::GLOBAL_HEADER);

    decoder.set_parameters(input.parameters())?;

    let mut output = octx.add_stream(codec)?;
    let context = ffmpeg::codec::context::Context::from_parameters(output.parameters())?;
    let mut encoder = context.encoder().audio()?;

    let channel_layout = codec
        .channel_layouts()
        .map(|cls| cls.best(decoder.channel_layout().channels()))
        .unwrap_or(ffmpeg::channel_layout::ChannelLayout::STEREO);

    if global {
        encoder.set_flags(ffmpeg::codec::flag::Flags::GLOBAL_HEADER);
    }

    encoder.set_rate(decoder.rate() as i32);
    encoder.set_channel_layout(channel_layout);
    encoder.set_format(
        codec
            .formats()
            .expect("unknown supported formats")
            .next()
            .unwrap(),
    );

    let scaled:f32 = linear_scale(file_size.to_owned(), 8.0, 500.0, 32.0, 128.0);
    let cool_bit_rate:f32 = scaled * 1024.0;

    encoder.set_bit_rate(cool_bit_rate as usize);
    encoder.set_max_bit_rate(cool_bit_rate as usize);

    encoder.set_time_base((1, decoder.rate() as i32));
    output.set_time_base((1, decoder.rate() as i32));

    let encoder = encoder.open_as(codec)?;
    output.set_parameters(&encoder);

    let filter = filter(filter_spec, &decoder, &encoder)?;

    let in_time_base = decoder.time_base();
    let out_time_base = output.time_base();

    Ok(Transcoder {
        stream: input.index(),
        filter,
        input_time_base: Rational::from((1, decoder.rate() as i32)),
        decoder,
        encoder,
        in_time_base,
        out_time_base,
        last_log_time: Instant::now(),
        last_log_frame_count: 0,
        starting_time: Instant::now(),
        frame_count: 0,
    })
}

impl Transcoder {
    fn send_frame_to_encoder(&mut self, frame: &ffmpeg::Frame) {
        self.encoder.send_frame(frame).unwrap();
    }

    fn send_eof_to_encoder(&mut self) {
        self.encoder.send_eof().unwrap();
    }

    fn receive_and_process_encoded_packets(&mut self, octx: &mut format::context::Output) {
        let mut encoded = ffmpeg::Packet::empty();
        while self.encoder.receive_packet(&mut encoded).is_ok() {
            encoded.set_stream(0);
            encoded.rescale_ts(self.in_time_base, self.out_time_base);
            encoded.write_interleaved(octx).unwrap();
        }
    }

    fn add_frame_to_filter(&mut self, frame: &ffmpeg::Frame) {
        self.filter.get("in").unwrap().source().add(frame).unwrap();
    }

    fn flush_filter(&mut self) {
        self.filter.get("in").unwrap().source().flush().unwrap();
    }

    fn get_and_process_filtered_frames(&mut self, octx: &mut format::context::Output) {
        let mut filtered = frame::Audio::empty();
        while self
            .filter
            .get("out")
            .unwrap()
            .sink()
            .frame(&mut filtered)
            .is_ok()
        {
            self.send_frame_to_encoder(&filtered);
            self.receive_and_process_encoded_packets(octx);
        }
    }

    fn send_packet_to_decoder(&mut self, packet: &ffmpeg::Packet) {
        self.decoder.send_packet(packet).unwrap();
    }

    fn send_eof_to_decoder(&mut self) {
        self.decoder.send_eof().unwrap();
    }

    fn receive_and_process_decoded_frames(&mut self, octx: &mut format::context::Output) {
        let mut decoded = frame::Audio::empty();
        while self.decoder.receive_frame(&mut decoded).is_ok() {
            self.frame_count += 1;
            let timestamp = decoded.timestamp();
            decoded.set_pts(timestamp);
            self.log_progress();
            self.add_frame_to_filter(&decoded);
            self.get_and_process_filtered_frames(octx);
        }
    }

    fn log_progress(&mut self) {
        if (self.frame_count - self.last_log_frame_count < 100 && self.last_log_time.elapsed().as_secs_f64() < 1.0) {
            return;
        }
        let total_seconds = self.starting_time.elapsed().as_secs_f64() as u64;
        let minutes = (total_seconds % 3600) / 60;
        let seconds = total_seconds % 60;
        let formatted_time = format!("{:02}:{:02}", minutes, seconds);
        eprintln!(
            "[RUST] AUDIO ELAPSED: \t{:8.2}s\tFRAMES: {:8}\tTIMESTAMP: \t{formatted_time}",
            self.starting_time.elapsed().as_secs_f64(),
            self.frame_count,
        );
        self.last_log_frame_count = self.frame_count;
        self.last_log_time = Instant::now();
    }
}

pub async fn audio(input: &PathBuf, file_size: &f32) -> PathBuf {
    let mut hasher = Sha1::new();
    let mut file = File::open(input).unwrap();

    let mut buffer = [0; 1024]; // 1 KB buffer
    loop {
        let bytes_read = file.read(&mut buffer).unwrap();
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }
    let result = hasher.finalize();
    let input_file_name = result.iter().map(|byte| format!("{:02x}", byte)).collect::<String>();
    
    let output_path = Path::new(&input)
        .parent()
        .expect("input file must have a parent directory")
        .join(input_file_name)
        .with_extension("aac");

    let output = output_path
        .to_str()
        .expect("failed to convert output file path to string");

    let filter = "anull".to_owned();
    let seek = 0;

    let mut ictx = format::input(&input).unwrap();
    let mut octx = format::output(&output).unwrap();
    let mut transcoder = transcoder(&mut ictx, &mut octx, &output, &filter, file_size).unwrap();

    octx.set_metadata(ictx.metadata().to_owned());
    octx.write_header().unwrap();

    for (stream, mut packet) in ictx.packets() {
        if stream.index() == transcoder.stream {
            packet.rescale_ts(stream.time_base(), transcoder.in_time_base);
            transcoder.send_packet_to_decoder(&packet);
            transcoder.receive_and_process_decoded_frames(&mut octx);
        }
    }

    transcoder.send_eof_to_decoder();
    transcoder.receive_and_process_decoded_frames(&mut octx);

    transcoder.flush_filter();
    transcoder.get_and_process_filtered_frames(&mut octx);

    transcoder.send_eof_to_encoder();
    transcoder.receive_and_process_encoded_packets(&mut octx);

    octx.write_trailer().unwrap();

    output_path
}
